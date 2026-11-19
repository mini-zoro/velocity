//! Durable pointers to sibling temporaries, including those interrupted
//! before a save reached its recovery journal.
use super::*;

#[derive(Debug)]
pub(crate) struct StageReference {
    path: PathBuf,
    stage: Option<PathBuf>,
    _lock: File,
    pub(crate) keep: bool,
}

impl Drop for StageReference {
    fn drop(&mut self) {
        if !self.keep {
            if let Some(stage) = &self.stage {
                let _ = remove_companion(stage);
            }
            let _ = fs::remove_file(&self.path);
        }
    }
}

#[derive(Deserialize, Serialize)]
struct Reference {
    file: JournalPath,
    // Old records stamped the inode when the file was empty. FAT/exFAT can
    // change it once data is written. The reserved, locked name is authoritative.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    inode: Option<Entry>,
    target: Option<Entry>,
}

pub(in crate::save_journal) fn new(
    root: &Path,
    directory: &Path,
    mode: Option<fs::Permissions>,
    target: Option<Entry>,
) -> io::Result<Stage> {
    ensure_directory(root)?;
    let mut record = Stage::new(root, &preparation_prefix(REFERENCE_TEMP), None)?;
    // tempfile uses an exclusive, CLOEXEC open; try_clone is CLOEXEC as well.
    let lock = record.descriptor().try_clone()?;
    wait_for_lock(&lock, || lock.try_lock().map_err(Into::into))?;
    let token = record
        .path()
        .file_name()
        .and_then(|n| n.to_str())
        .and_then(|n| n.strip_prefix(REFERENCE_TEMP))
        .expect("generated reference name");
    let stage_path = directory.join(format!("{STAGE}{token}"));
    let value = Reference {
        file: JournalPath::new(&stage_path),
        inode: None,
        target,
    };
    write_record(record.file(), &value)?;
    let path = reference_path(root, &stage_path).expect("save stage name");
    let cross_volume = fs::metadata(root)?.dev() != fs::metadata(directory)?.dev();
    // Publish before the stage exists. The private reference remains locked
    // through creation, publication of the save and cleanup.
    if let Some(warning) = record.publish(&path, true)?.warning {
        return Err(io::Error::other(warning));
    }
    let mut reference = StageReference {
        path,
        stage: None,
        _lock: lock,
        keep: false,
    };
    atomic::sync(
        &reference._lock,
        if cross_volume {
            Durability::Full
        } else {
            Durability::Barrier
        },
    )?;
    fault(Phase::BeforeStage)?;
    let mut stage = Stage::create_at(&stage_path, mode)?;
    reference.stage = Some(stage_path);
    stage.track(reference);
    fault(Phase::AfterStage)?;
    Ok(stage)
}

pub(crate) fn tracked_stage(
    profile: Option<&Path>,
    directory: &Path,
    mode: Option<fs::Permissions>,
) -> io::Result<Stage> {
    let root = std::path::absolute(recovery_dir(profile))?;
    new(&root, directory, mode, None)
}

pub(crate) fn clean_stages(profile: Option<&Path>, directory: &Path) -> Vec<String> {
    clean(&recovery_dir(profile), Some(directory))
}

fn reference_path(root: &Path, stage: &Path) -> Option<PathBuf> {
    let name = stage.file_name()?.to_str()?.strip_prefix(STAGE)?;
    (!name.is_empty() && name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-'))
        .then(|| root.join(format!("{name}.save-stage")))
}

pub(super) fn forget(root: &Path, stage: &Path) -> io::Result<()> {
    if let Some(reference) = reference_path(root, stage) {
        match fs::remove_file(reference) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => (),
            result => result?,
        }
    }
    Ok(())
}

pub(super) fn open_owned(path: &Path) -> Result<Option<(File, Entry)>, String> {
    let error = |e| regular::error(path, e);
    let current = match fs::symlink_metadata(path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        result => result.map_err(error)?,
    };
    if !current.is_file() || current.uid() != rustix::process::geteuid().as_raw() {
        return Err(regular::error(
            path,
            "not a user-owned regular staging file; retained",
        ));
    }
    let file = regular::read(path)?;
    let entry = Entry::of(&current);
    check_entry(path, &file, entry)?;
    Ok(Some((file, entry)))
}

fn remove_owned(path: &Path) -> Result<bool, String> {
    let Some((_file, _entry)) = open_owned(path)? else {
        return Ok(false);
    };
    fs::remove_file(path).map_err(|e| regular::error(path, e))?;
    Ok(true)
}

pub(super) fn remove_companion(stage: &Path) -> Result<(), String> {
    let mut name = std::ffi::OsString::from("._");
    name.push(stage.file_name().expect("stage filename"));
    remove_owned(&stage.with_file_name(name))?;
    Ok(())
}

fn reap_preparation(path: &Path) -> Result<(), String> {
    let Some((file, entry)) = open_owned(path)? else {
        return Ok(());
    };
    let pid = path
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| {
            name.strip_prefix(JOURNAL_TEMP)
                .or_else(|| name.strip_prefix(REFERENCE_TEMP))
                .or_else(|| name.strip_prefix(BACKUP))
                .or_else(|| name.strip_prefix(PRIVATE_STAGE))
                .or_else(|| name.strip_prefix(PROOF))
        })
        .and_then(|name| name.split_once('-'))
        .and_then(|(pid, _)| pid.parse::<i32>().ok())
        .filter(|pid| *pid > 0)
        .and_then(rustix::process::Pid::from_raw);
    let backup = staged_name(path, BACKUP);
    let private_stage = staged_name(path, PRIVATE_STAGE) || staged_name(path, PROOF);
    let abandoned = pid.map_or_else(
        || {
            // For old temp names without a pid, avoid racing the creator
            // between open and flock.
            file.metadata()
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|time| time.elapsed().ok())
                .is_some_and(|age| age >= Duration::from_secs(if backup { 86_400 } else { 60 }))
        },
        |pid| {
            let old_enough = file.metadata().ok().is_some_and(|m| {
                u64::try_from(m.ctime())
                    .ok()
                    .and_then(|secs| SystemTime::UNIX_EPOCH.checked_add(Duration::from_secs(secs)))
                    .is_some_and(|time| SystemTime::now() > time)
            });
            old_enough && rustix::process::test_kill_process(pid) == Err(rustix::io::Errno::SRCH)
        },
    );
    if abandoned && file.try_lock().is_ok() {
        check_entry(path, &file, entry)?;
        if (backup || private_stage) && referenced_preparation(path) {
            return Ok(());
        }
        fs::remove_file(path).map_err(|e| regular::error(path, e))?;
        if private_stage {
            remove_companion(path)?;
        }
    }
    Ok(())
}

fn referenced_preparation(path: &Path) -> bool {
    let root = atomic::parent(path);
    let Ok(mut records) = fs::read_dir(root) else {
        return true;
    };
    records.any(|record| {
        let Ok(record) = record else { return true };
        let record = record.path();
        if staged_name(&record, retained::PREFIX) {
            return false;
        }
        if record.extension().is_some_and(|e| e == "save-journal") {
            return read_journal(root, &record).map_or(true, |(_, journal)| {
                journal.stage.get() == path
                    || journal
                        .original
                        .as_ref()
                        .and_then(Original::pages)
                        .is_some_and(|p| p.path() == path)
                    || journal
                        .rollback
                        .as_ref()
                        .is_some_and(|backup| backup.get() == path)
            });
        }
        #[cfg(target_os = "macos")]
        if (staged_name(path, PRIVATE_STAGE) || staged_name(path, PROOF))
            && record.extension().is_some_and(|e| e == "save-undo")
        {
            return undo::refers_to(&record, path).unwrap_or(true);
        }
        false
    })
}

pub(super) fn clean(root: &Path, only: Option<&Path>) -> Vec<String> {
    // This is a scan of the private profile, never of the user's project.
    if let Err(e) = check_directory(root) {
        return if e.kind() == io::ErrorKind::NotFound {
            Vec::new()
        } else {
            vec![regular::error(root, e)]
        };
    }
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(e) => return vec![regular::error(root, e)],
    };
    entries
        .filter_map(|entry| match entry {
            Ok(entry) if entry.path().extension().is_some_and(|e| e == "save-stage") => {
                reap(root, &entry.path(), only).unwrap_or_else(Some)
            }
            Ok(entry)
                if only.is_none()
                    && [JOURNAL_TEMP, REFERENCE_TEMP, BACKUP, PRIVATE_STAGE, PROOF]
                        .iter()
                        .any(|prefix| {
                            entry
                                .file_name()
                                .as_encoded_bytes()
                                .starts_with(prefix.as_bytes())
                        }) =>
            {
                reap_preparation(&entry.path()).err()
            }
            Err(e) => Some(regular::error(root, e)),
            _ => None,
        })
        .collect()
}

fn reap(root: &Path, record: &Path, only: Option<&Path>) -> Result<Option<String>, String> {
    let raw = match regular::metadata(record, true)? {
        Some(_) => regular::read(record)?,
        None => return Ok(None),
    };
    if raw.try_lock().is_err() {
        return Ok(None);
    }
    let entry = Entry::of(&raw.metadata().map_err(|e| regular::error(record, e))?);
    check_entry(record, &raw, entry)?;
    let reference: Reference =
        serde_json::from_reader((&raw).take(MAX_RECORD)).map_err(|e| regular::error(record, e))?;
    let path = reference.file.get();
    if !path.is_absolute() || reference_path(root, path).as_deref() != Some(record) {
        return Err(regular::error(
            record,
            "invalid staging reference; retained",
        ));
    }
    if only.is_some_and(|parent| atomic::parent(path) != parent)
        && only.is_some_and(|_| path.parent() != Some(root))
    {
        return Ok(None);
    }
    let protected = || -> io::Result<bool> {
        let Some(target) = reference.target else {
            return Ok(false);
        };
        if pending_file(&root.join(target.record_name())).map_err(io::Error::other)? {
            return Ok(true);
        }
        #[cfg(target_os = "macos")]
        if pending_file(&root.join(undo::name(target))).map_err(io::Error::other)? {
            return Ok(true);
        }
        Ok(false)
    };
    if protected().map_err(|e| regular::error(record, e))? {
        return Ok(None);
    }
    let removed = remove_owned(path)?;
    remove_companion(path)?;
    forget(root, path).map_err(|e| regular::error(record, e))?;
    Ok(removed.then(|| format!("Removed incomplete save stage: {}", path.display())))
}
