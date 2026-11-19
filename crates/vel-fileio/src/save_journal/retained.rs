use super::*;

pub(super) const PREFIX: &str = "Recovered-";

pub(super) fn open_verified(
    path: &Path,
    journal: &Journal,
    content: Option<ContentIdentity>,
) -> Result<File, String> {
    let (file, _) = stages::open_owned(path)?
        .ok_or_else(|| regular::error(path, io::Error::from(io::ErrorKind::NotFound)))?;
    let error = |e| regular::error(path, e);
    let metadata = file.metadata().map_err(error)?;
    if metadata.len() != journal.stage_len {
        return Err(regular::error(path, "staged size changed"));
    }
    if let Some(content) = content {
        if proof::checksum(&file, metadata.len()).map_err(error)? != content {
            return Err(regular::error(
                path,
                "staged checksum changed; retained for manual recovery",
            ));
        }
    } else if !journal
        .stage_entry
        .same_file(Entry::on_volume(&metadata, &volume::inspect(&file)))
    {
        return Err(regular::error(path, "staging file identity changed"));
    }
    check_recovery_revision(path, &file, &metadata)?;
    Ok(file)
}

pub(super) fn archive(
    root: &Path,
    record: &Path,
    journal: &Journal,
    reason: &str,
) -> Result<RecoveryAction, String> {
    let target = journal.target.get();
    let stage = journal.stage.get();
    let error = |e| regular::error(record, e);
    // Stable across retries, including a crash after publishing the recovered
    // file but before removing the interrupted save record.
    let stamp = fs::symlink_metadata(record)
        .and_then(|m| m.modified())
        .map_err(error)?;
    let nonce = stage
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .chars()
        .skip(STAGE.len())
        .take(40)
        .collect::<String>();
    let archive = recovery_name(root, target, stamp, &nonce);
    let archived_file = if regular::metadata(&archive, true)?.is_some() {
        let content = if journal.content.is_none() && regular::metadata(stage, true)?.is_some() {
            let source = open_verified(stage, journal, None)?;
            Some(proof::checksum(&source, journal.stage_len).map_err(error)?)
        } else {
            journal.content
        };
        open_verified(&archive, journal, content)?
    } else {
        let source = open_verified(stage, journal, journal.content)?;
        match fault(Phase::ArchiveRename).and_then(|()| crate::exclusive_rename(stage, &archive)) {
            Ok(()) => source,
            Err(e) if atomic::unsupported(&e) || e.raw_os_error() == Some(libc::EXDEV) => {
                let content = match journal.content {
                    Some(content) => content,
                    None => proof::checksum(&source, journal.stage_len).map_err(error)?,
                };
                // Copy exactly the checked length with a bounded buffer. A
                // failed copy never consumes the source or its journal.
                let mut copy = stages::new(root, root, None, None).map_err(error)?;
                copy_data(&source, copy.file(), journal.stage_len, false).map_err(error)?;
                if proof::checksum(copy.descriptor(), journal.stage_len).map_err(error)? != content
                {
                    return Err(regular::error(
                        stage,
                        "staged content changed during recovery",
                    ));
                }
                copy.sync(Durability::Full).map_err(error)?;
                let file = copy.descriptor().try_clone().map_err(error)?;
                let _ = copy.publish(&archive, true).map_err(error)?;
                file
            }
            Err(e) => return Err(error(e)),
        }
    };
    check_entry(
        &archive,
        &archived_file,
        Entry::of(&archived_file.metadata().map_err(error)?),
    )?;
    atomic::sync_parent(&archive).map_err(error)?;
    atomic::sync(&archived_file, Durability::Full).map_err(error)?;
    let mut message = format!(
        "{}: {reason}; no data was overwritten. Staged revision kept at {}",
        target.display(),
        archive.display(),
    );
    let original = if let Some(backup) = &journal.rollback
        && let Some((file, _)) = stages::open_owned(backup.get())?
    {
        let len = file.metadata().map_err(error)?.len();
        if journal.original_len.is_some_and(|expected| expected != len) {
            return Err(regular::error(backup.get(), "saved original size changed"));
        }
        Some(archive_image(
            record,
            target,
            Image::file(&file, 0, len).link(backup.get()),
            "original",
        )?)
    } else if let Some(Original::Inline(bytes)) = &journal.original {
        Some(archive_image(
            record,
            target,
            Image::Memory(bytes),
            "original",
        )?)
    } else {
        None
    };
    if let Some(original) = original {
        message.push_str(&format!("; original kept at {}", original.display()));
    }
    let cleanup = fault(Phase::ArchivePublished).and_then(|()| discard(record, journal, true));
    if let Err(e) = cleanup {
        message.push_str(&format!("; save recovery cleanup failed: {e}"));
    }
    Ok(RecoveryAction {
        message: Some(message),
        verified: false,
    })
}

fn recovery_name(root: &Path, target: &Path, stamp: SystemTime, nonce: &str) -> PathBuf {
    let time = stamp
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let name = target.file_name().unwrap_or_default().to_string_lossy();
    let name: String = name
        .chars()
        .take(40)
        .map(|c| if c.is_control() || c == ':' { '_' } else { c })
        .collect();
    root.join(format!("{PREFIX}{time}-{nonce}-{name}"))
}

pub(super) enum Image<'a> {
    File {
        file: &'a File,
        offset: u64,
        len: u64,
        link: Option<&'a Path>,
    },
    Memory(&'a [u8]),
}

impl<'a> Image<'a> {
    pub(super) fn file(file: &'a File, offset: u64, len: u64) -> Self {
        Self::File {
            file,
            offset,
            len,
            link: None,
        }
    }

    pub(super) fn link(mut self, path: &'a Path) -> Self {
        if let Self::File { link, .. } = &mut self {
            *link = Some(path);
        }
        self
    }
}

pub(super) fn archive_image(
    record: &Path,
    target: &Path,
    image: Image<'_>,
    kind: &str,
) -> Result<PathBuf, String> {
    let error = |e| regular::error(record, e);
    let metadata = fs::symlink_metadata(record).map_err(error)?;
    let root = atomic::parent(record);
    let archive = recovery_name(
        root,
        target,
        metadata.modified().map_err(error)?,
        &format!("{:016x}-{kind}", metadata.ino()),
    );
    let saved = if let Some((actual, _)) = stages::open_owned(&archive)? {
        // A previous recovery might have published this copy before dying.
        let expected = match image {
            Image::File {
                file, offset, len, ..
            } => proof::checksum_range(file, offset, len).map_err(error)?,
            Image::Memory(bytes) => ContentIdentity {
                bytes: bytes.len() as u64,
                hash: *blake3::hash(bytes).as_bytes(),
            },
        };
        if actual.metadata().map_err(error)?.len() != expected.bytes
            || proof::checksum(&actual, expected.bytes).map_err(error)? != expected
        {
            return Err(error(io::Error::other(
                "recovered file already exists and differs",
            )));
        }
        actual
    } else if let Image::File {
        file,
        link: Some(link),
        ..
    } = image
        && fs::hard_link(link, &archive).is_ok()
    {
        // Keep the journal available if publication fails.
        file.try_clone().map_err(error)?
    } else {
        let mut output =
            Stage::new(root, &preparation_prefix(JOURNAL_TEMP), None).map_err(error)?;
        match image {
            Image::File {
                file,
                mut offset,
                mut len,
                ..
            } => {
                let mut chunk = vec![0; len.min(1024 * 1024) as usize];
                while len != 0 {
                    let count = len.min(chunk.len() as u64) as usize;
                    file.read_exact_at(&mut chunk[..count], offset)
                        .map_err(error)?;
                    output.file().write_all(&chunk[..count]).map_err(error)?;
                    offset += count as u64;
                    len -= count as u64;
                }
            }
            Image::Memory(bytes) => output.file().write_all(bytes).map_err(error)?,
        }
        let saved = output.descriptor().try_clone().map_err(error)?;
        let _ = output.publish(&archive, true).map_err(error)?;
        saved
    };
    atomic::sync_parent(&archive).map_err(error)?;
    atomic::sync(&saved, Durability::Full).map_err(error)?;
    Ok(archive)
}
