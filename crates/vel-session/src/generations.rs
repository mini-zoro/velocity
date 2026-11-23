use super::{Document, History, Recovery, Session, Sheet, ViewState, Window};
use serde::de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor};
use std::{
    ffi::OsStr,
    fs,
    io::{self, BufWriter, Read, Write},
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use vel_fileio::{
    atomic::{self, Durability, Stage},
    regular,
};
use vel_text::Text;

const STAGE_PREFIX: &str = ".velocity-session-";

fn previous(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".1");
    name.into()
}

fn read(path: &Path) -> Result<Option<Vec<u8>>, String> {
    if regular::metadata(path, true)?.is_none() {
        return Ok(None);
    }
    let mut bytes = Vec::new();
    regular::read(path)?
        .read_to_end(&mut bytes)
        .map_err(|e| regular::error(path, e))?;
    Ok(Some(bytes))
}

fn decode(bytes: &[u8]) -> Result<Session, String> {
    let session: Session = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    session.validate()?;
    Ok(session)
}

pub(super) fn load(path: &Path) -> Recovery {
    if path.file_name() == Some(OsStr::new("Session.sublime-session")) {
        clean_stale(
            atomic::parent(path),
            STAGE_PREFIX,
            Duration::from_secs(24 * 3600),
        );
    }
    recover(path, Some(&previous(path)), true)
}

pub(super) struct Project {
    prefix: String,
    previous: Option<PathBuf>,
}

// The public project documentation describes what Sublime stores, but does
// not specify its workspace JSON keys. Never publish a Velocity session over
// a well-formed JSON object that is not one of our session generations.
fn foreign_workspace(bytes: &[u8]) -> bool {
    use serde_json::value::RawValue;
    #[derive(serde::Deserialize)]
    struct Header<'a> {
        #[serde(borrow)]
        version: Option<&'a RawValue>,
        #[serde(borrow)]
        windows: Option<&'a RawValue>,
        #[serde(borrow)]
        documents: Option<&'a RawValue>,
    }
    bytes.iter().find(|byte| !byte.is_ascii_whitespace()) == Some(&b'{')
        && serde_json::from_slice::<Header<'_>>(bytes).is_ok_and(|header| {
            header.version.is_none_or(|value| value.get() != "1")
                || header.windows.is_none()
                || header.documents.is_none()
        })
}

impl Project {
    pub(super) fn new(project: &Path, data: Option<&Path>) -> Self {
        let path = fs::canonicalize(project)
            .or_else(|_| std::path::absolute(project))
            .unwrap_or_else(|_| project.to_owned());
        let key = blake3::hash(path.as_os_str().as_encoded_bytes());
        Self {
            prefix: format!("{STAGE_PREFIX}{key}-"),
            previous: data.map(|data| {
                data.join("Sessions")
                    .join(format!("Project-{key}.previous"))
            }),
        }
    }

    pub(super) fn load(&self, path: &Path) -> Recovery {
        let age = Duration::from_secs(24 * 3600);
        clean_stale(atomic::parent(path), &self.prefix, age);
        if let Some(previous) = &self.previous {
            clean_stale(atomic::parent(previous), &self.prefix, age);
        }
        if let Ok(Some(bytes)) = read(path)
            && foreign_workspace(&bytes)
        {
            let mut recovery = recover(
                &path.with_extension("velocity-workspace"),
                self.previous.as_deref(),
                false,
            );
            if recovery.source.is_none() {
                recovery.messages.push(format!(
                    "Workspace format not recognized; kept {} unchanged",
                    path.display()
                ));
            }
            recovery
        } else {
            recover(path, self.previous.as_deref(), false)
        }
    }
}

fn recover(path: &Path, previous: Option<&Path>, quarantine_primary: bool) -> Recovery {
    let mut result = Recovery::default();
    let mut damaged = Vec::new();
    for candidate in [Some(path), previous].into_iter().flatten() {
        match read(candidate) {
            Ok(Some(bytes)) => match decode(&bytes) {
                Ok(session) => {
                    if candidate != path {
                        result.messages.push(format!(
                            "Restored previous session from {}",
                            candidate.display()
                        ));
                    }
                    result.source = Some(candidate.to_owned());
                    result.session = session;
                    return result;
                }
                Err(e) => {
                    let suffix = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_nanos();
                    let aside = candidate.with_extension(format!("corrupt-{suffix}"));
                    let msg = if candidate == path && !quarantine_primary {
                        format!("Corrupt project workspace {}: {e}", candidate.display())
                    } else {
                        match vel_fileio::rename_no_replace(candidate, &aside) {
                            Ok(()) => format!("Corrupt session kept at {}: {e}", aside.display()),
                            Err(io) => format!(
                                "Corrupt session {} (could not quarantine: {io}): {e}",
                                candidate.display()
                            ),
                        }
                    };
                    result.messages.push(msg);
                    damaged.push(bytes);
                }
            },
            Err(e) => result.messages.push(e),
            Ok(None) => {}
        }
    }
    result.session = salvage(&damaged);
    if !result.session.documents.is_empty() {
        result.messages.push(format!(
            "Recovered text from {} session document(s); save the recovered tabs separately.",
            result.session.documents.len()
        ));
    }
    result
}

fn clean_stale(parent: &Path, prefix: &str, age: Duration) {
    let Ok(entries) = parent.read_dir() else {
        return;
    };
    let now = SystemTime::now();
    for item in entries.flatten() {
        let name = item.file_name();
        let name = name.to_string_lossy();
        // Old tempfile used .tmp + six random characters in Sessions only.
        let old_stage = prefix == STAGE_PREFIX
            && name
                .strip_prefix(".tmp")
                .is_some_and(|s| s.len() == 6 && s.bytes().all(|b| b.is_ascii_alphanumeric()));
        if !(old_stage || name.starts_with(prefix)) {
            continue;
        }
        if item
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| now.duration_since(t).ok())
            .is_some_and(|elapsed| elapsed >= age)
        {
            match item.file_type() {
                Ok(ty) if ty.is_file() => {
                    let _ = fs::remove_file(item.path());
                }
                // Only our reservation directories (never symlinks or old .tmp dirs).
                Ok(ty) if ty.is_dir() && name.starts_with(STAGE_PREFIX) => {
                    let _ = fs::remove_file(item.path().join("previous"));
                    let _ = fs::remove_dir(item.path());
                }
                _ => (),
            }
        }
    }
}

#[derive(Default)]
pub(super) struct Saved {
    known: Option<(super::DiskStamp, blake3::Hash)>,
    unreadable: Option<Option<super::DiskStamp>>,
    pub(super) warnings: Vec<String>,
    pub(super) project: Option<Project>,
}

impl Saved {
    // https://www.sublimetext.com/download (4126): avoid losing the session to
    // an interrupted save. Keep a prior valid checkpoint before publishing.
    pub(super) fn write(&mut self, path: &Path, session: &Session) -> Result<(), String> {
        if self.project.is_some()
            && path.extension() == Some(OsStr::new("sublime-workspace"))
            && self
                .known
                .as_ref()
                .is_none_or(|(stamp, _)| super::DiskStamp::read(path).as_ref() != Some(stamp))
            && read(path)?.is_some_and(|bytes| foreign_workspace(&bytes))
        {
            return self.write(&path.with_extension("velocity-workspace"), session);
        }
        session.validate()?;
        let parent = atomic::parent(path);
        let error = |e| regular::error(path, e);
        fs::create_dir_all(parent).map_err(error)?;
        let (previous, prefix) = if let Some(project) = &self.project {
            (project.previous.clone(), project.prefix.as_str())
        } else {
            (Some(previous(path)), STAGE_PREFIX)
        };
        let staging = previous.as_deref().map(atomic::parent).unwrap_or(parent);
        fs::create_dir_all(staging).map_err(error)?;
        let staging = if same_filesystem(staging, parent) {
            staging
        } else {
            parent
        };
        let mut stage = Stage::new(staging, prefix, None).map_err(error)?;
        let mut digest = HashWriter {
            output: BufWriter::with_capacity(64 * 1024, stage.file()),
            hash: blake3::Hasher::new(),
        };
        serde_json::to_writer(&mut digest, session).map_err(|e| regular::error(path, e))?;
        digest.flush().map_err(error)?;
        let hash = digest.hash.finalize();
        drop(digest);

        let existing = super::DiskStamp::read(path);
        let cached = self
            .known
            .as_ref()
            .filter(|(stamp, _)| Some(stamp) == existing.as_ref());
        let current = if cached.is_some() {
            // A chmod/ACL change can leave the content stamp the same.
            regular::read(path).map(|_| true)
        } else {
            read(path).map(|value| value.is_some_and(|bytes| decode(&bytes).is_ok()))
        };
        let valid = match current {
            Ok(valid) => valid,
            Err(e) => {
                if self.unreadable.as_ref() != Some(&existing) {
                    self.warnings
                        .push(format!("Skipping unreadable session generation: {e}"));
                    self.unreadable = Some(existing);
                }
                false
            }
        };
        if valid && cached.is_some_and(|(_, previous_hash)| *previous_hash == hash) {
            return Ok(());
        }
        stage.sync(Durability::Barrier).map_err(error)?;
        if valid && let Some(previous) = previous {
            preserve_previous(path, &previous)?;
        }
        if let Some(warning) = stage.publish(path, false).map_err(error)?.warning {
            self.warnings.push(warning);
        }
        self.known = super::DiskStamp::read(path).map(|stamp| (stamp, hash));
        Ok(())
    }
}

#[cfg(unix)]
fn same_filesystem(a: &Path, b: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    a == b
        || fs::metadata(a)
            .ok()
            .zip(fs::metadata(b).ok())
            .is_some_and(|(a, b)| a.dev() == b.dev())
}

#[cfg(not(unix))]
fn same_filesystem(a: &Path, b: &Path) -> bool {
    a == b
}

fn preserve_previous(path: &Path, destination: &Path) -> Result<(), String> {
    let parent = atomic::parent(destination);
    let reserve = tempfile::Builder::new()
        .prefix(STAGE_PREFIX)
        .tempdir_in(parent)
        .map_err(|e| regular::error(destination, e))?;
    let link = reserve.path().join("previous");
    if atomic::parent(path) == parent
        && fs::symlink_metadata(path).is_ok_and(|m| m.is_file())
        && fs::hard_link(path, &link).is_ok()
    {
        fs::rename(link, destination).map_err(|e| regular::error(destination, e))?;
        // Directory entries are made durable with a directory fsync on Unix only; Windows
        // cannot open a directory as a file (Access is denied on every checkpoint), and
        // NTFS journals the rename itself (as vel-fileio's own parent sync does).
        #[cfg(unix)]
        if let Err(e) =
            fs::File::open(parent).and_then(|file| atomic::sync(&file, Durability::Barrier))
        {
            eprintln!("{}: {e}", destination.display());
        }
    } else {
        // Network and FAT may not support hard links. The old file still
        // exists until the complete fallback copy has been committed.
        let mut source = regular::read(path)?;
        super::atomic_write(destination, |out| {
            io::copy(&mut source, out)
                .map(|_| ())
                .map_err(|e| regular::error(destination, e))
        })?;
    }
    Ok(())
}

struct HashWriter<W> {
    output: W,
    hash: blake3::Hasher,
}
impl<W: Write> Write for HashWriter<W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let len = self.output.write(bytes)?;
        self.hash.update(&bytes[..len]);
        Ok(len)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.output.flush()
    }
}

fn salvage(damaged: &[Vec<u8>]) -> Session {
    let mut session = Session::default();
    let mut window = Window::default();
    let mut seen = std::collections::BTreeSet::new();
    for bytes in damaged {
        let mut candidates = Vec::new();
        let mut json = serde_json::Deserializer::from_slice(bytes);
        // Keep complete text strings even when the following history is truncated.
        // No undo, saved state or document paths are restored.
        let _ = RecoverDocuments(&mut candidates, false).deserialize(&mut json);
        for document in candidates {
            let text = document
                .text
                .as_deref()
                .expect("only complete text is collected");
            let name = document
                .path
                .as_deref()
                .and_then(|p| Path::new(p).file_name())
                .and_then(OsStr::to_str)
                .unwrap_or("Untitled");
            if session.documents.len() == 10_000
                || !seen.insert((
                    name.to_owned(),
                    blake3::hash(text.as_bytes()).as_bytes().to_owned(),
                ))
            {
                continue;
            }
            let id = session.documents.len() as u64 + 1;
            let buffer = vel_buffer::Buffer::new(Text::from(text));
            session.documents.push(Document {
                id,
                path: None,
                disk: None,
                disk_content: None,
                saved_in_effective_encoding: false,
                effective_encoding: None,
                encoding: "UTF-8".into(),
                encoding_chosen: false,
                preferred_encoding: None,
                encoding_display_name: None,
                bom: false,
                line_ending: "LF".into(),
                dirty: true,
                history: Some(History::recovered(&buffer, format!("Recovered {name}"))),
            });
            window.sheets.push(Sheet {
                settings: None,
                html: None,
                id,
                document: id,
                view: ViewState::default(),
                syntax: None,
                transient: false,
                name: None,
            });
            window.workbench.groups[0].add(id, false);
        }
    }
    if !window.sheets.is_empty() {
        window.workbench.groups[0].click(window.sheets[0].id, false, false);
        session.windows.push(window);
    }
    session
}

#[derive(Default)]
struct PartialDocument {
    path: Option<String>,
    text: Option<String>,
}

struct RecoverDocuments<'a>(&'a mut Vec<PartialDocument>, bool);
impl<'de> DeserializeSeed<'de> for RecoverDocuments<'_> {
    type Value = ();
    fn deserialize<D: de::Deserializer<'de>>(self, de: D) -> Result<(), D::Error> {
        if self.1 {
            de.deserialize_seq(self)
        } else {
            de.deserialize_map(self)
        }
    }
}
impl<'de> Visitor<'de> for RecoverDocuments<'_> {
    type Value = ();
    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("a session document array")
    }
    fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<(), M::Error> {
        while let Some(key) = map.next_key::<String>()? {
            if key == "documents" {
                map.next_value_seed(RecoverDocuments(self.0, true))?;
            } else {
                map.next_value::<de::IgnoredAny>()?;
            }
        }
        Ok(())
    }
    fn visit_seq<S: SeqAccess<'de>>(self, mut seq: S) -> Result<(), S::Error> {
        while self.0.len() < 10_000 {
            let mut document = PartialDocument::default();
            let parsed = seq.next_element_seed(DocumentFields(&mut document, false));
            if document.text.is_some() {
                self.0.push(document);
            }
            if parsed?.is_none() {
                break;
            }
        }
        Ok(())
    }
}

struct DocumentFields<'a>(&'a mut PartialDocument, bool);
impl<'de> DeserializeSeed<'de> for DocumentFields<'_> {
    type Value = ();
    fn deserialize<D: de::Deserializer<'de>>(self, de: D) -> Result<(), D::Error> {
        de.deserialize_any(self)
    }
}
impl<'de> Visitor<'de> for DocumentFields<'_> {
    type Value = ();
    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("session document fields")
    }
    fn visit_unit<E: de::Error>(self) -> Result<(), E> {
        Ok(())
    }
    fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<(), M::Error> {
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "text" => {
                    let value = map.next_value::<serde_json::Value>()?;
                    if let serde_json::Value::String(text) = value {
                        self.0.text = Some(text);
                    }
                }
                "path" if !self.1 => {
                    let value = map.next_value::<serde_json::Value>()?;
                    if let serde_json::Value::String(path) = value {
                        self.0.path = Some(path);
                    }
                }
                "history" if !self.1 => map.next_value_seed(DocumentFields(self.0, true))?,
                _ => {
                    map.next_value::<de::IgnoredAny>()?;
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn version(name: &str) -> Session {
        Session {
            recent_files: vec![PathBuf::from(name)],
            ..Session::default()
        }
    }

    #[test]
    fn previous_valid_generation_recovers_and_identical_checkpoint_does_not_rotate() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("Session.sublime-session");
        let mut writer = Saved::default();
        writer.write(&path, &version("first")).unwrap();
        writer.write(&path, &version("second")).unwrap();
        let first = fs::read(previous(&path)).unwrap();
        let second = fs::read(&path).unwrap();
        writer.write(&path, &version("second")).unwrap();
        assert_eq!(fs::read(previous(&path)).unwrap(), first);
        assert_eq!(fs::read(&path).unwrap(), second);
        fs::write(&path, &second[..second.len() / 2]).unwrap();
        let restored = load(&path);
        assert_eq!(restored.session.recent_files, [PathBuf::from("first")]);
        assert_eq!(restored.source, Some(previous(&path)));
        assert!(
            restored
                .messages
                .iter()
                .any(|m| m.contains("previous session"))
        );
    }

    #[test]
    fn both_corrupt_extract_complete_text_and_only_remove_stale_stage_names() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("Session.sublime-session");
        fs::write(
            &path,
            r#"{"version":1,"documents":[{"path":"/old/a.txt","history":{"text":"Unsaved 😀","snapshot":{"#,
        )
        .unwrap();
        fs::write(
            previous(&path),
            br#"{"version":1,"documents":[{"path":"/old/b.txt","text":"Older"}]"#,
        )
        .unwrap();
        let restored = load(&path);
        restored.session.validate().unwrap();
        assert_eq!(restored.session.documents.len(), 2);
        let doc = &restored.session.documents[0];
        assert!(doc.dirty && doc.path.is_none());
        let history = doc.history.as_ref().unwrap();
        assert_eq!(history.recovered_name(), Some("Recovered a.txt"));
        assert_eq!(history.current_text().unwrap().to_string(), "Unsaved 😀");
        let stage = tmp.path().join(".velocity-session-crashed");
        let document = tmp.path().join(".tmp-my-notes");
        fs::write(&stage, "partial").unwrap();
        fs::write(&document, "keep").unwrap();
        clean_stale(tmp.path(), STAGE_PREFIX, Duration::ZERO);
        assert!(!stage.exists() && document.exists());
    }

    #[cfg(unix)]
    #[test]
    fn flush_replaces_unreadable_checkpoint_without_rotating_it() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("session");
        let writer = crate::Writer::new(path.clone(), Duration::from_secs(100)).unwrap();
        for name in ["older", "latest"] {
            writer.changed(version(name)).unwrap();
            writer.flush().unwrap();
        }
        fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).unwrap();
        assert!(
            regular::read(&path).is_err(),
            "requires unprivileged fixture user"
        );
        writer.changed(version("new checkpoint")).unwrap();
        writer.flush().unwrap();
        assert!(writer.warnings.try_recv().unwrap().contains("unreadable"));
        assert!(writer.warnings.try_recv().is_err());
        assert_eq!(
            load(&path).session.recent_files,
            [PathBuf::from("new checkpoint")]
        );
        assert_eq!(
            load(&previous(&path)).session.recent_files,
            [PathBuf::from("older")]
        );
    }

    #[test]
    fn stale_reservations_release_session_hard_links() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("session");
        fs::write(&path, b"published").unwrap();
        let reservation = root.path().join(".velocity-session-reservation");
        fs::create_dir(&reservation).unwrap();
        fs::hard_link(&path, reservation.join("previous")).unwrap();
        let unrelated = root.path().join(".tmp-not-ours");
        fs::create_dir(&unrelated).unwrap();
        clean_stale(root.path(), STAGE_PREFIX, Duration::ZERO);
        assert!(!reservation.exists());
        assert!(unrelated.exists());
        assert_eq!(fs::read(path).unwrap(), b"published");
    }

    #[test]
    fn session_writer_makes_progress_during_continuous_changes() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("session");
        let writer = super::super::Writer::with_max_wait(
            path.clone(),
            Duration::from_millis(200),
            Duration::from_millis(600),
        )
        .unwrap();
        let started = Instant::now();
        let mut writes = Vec::new();
        for tick in 0..60 {
            writer.changed(version(&format!("change-{tick}"))).unwrap();
            for result in writer.results.try_iter() {
                result.unwrap();
                writes.push(started.elapsed());
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(writes.len() >= 3, "{writes:?}");
        // Bounded by max_wait (600 ms) plus scheduling; a loaded CI runner took 830 ms.
        assert!(writes[0] <= Duration::from_secs(3), "{writes:?}");
        writer.flush().unwrap();
        assert_eq!(
            load(&path).session.recent_files,
            [PathBuf::from("change-59")]
        );
    }

    #[test]
    fn failed_checkpoint_callback_never_publishes_a_partial_session() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("session");
        super::super::save(&path, &version("original")).unwrap();
        assert!(
            super::super::atomic_write(&path, |file| {
                file.write_all(b"partial").unwrap();
                Err("injected quota error".into())
            })
            .is_err()
        );
        assert_eq!(
            load(&path).session.recent_files,
            [PathBuf::from("original")]
        );
    }
}
