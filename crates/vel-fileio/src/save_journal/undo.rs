//! Inode-preserving rollback. Small saves keep both revisions in one private
//! file; larger APFS saves keep a COW clone and hashes of the intended revision.
pub(super) mod inline;
use super::*;
use crate::save::Saved;
use std::io::Write;

const ATTR: &str = "com.velocity.save-journal";

pub(super) fn name(entry: Entry) -> String {
    format!("{}-{:016x}.save-undo", entry.mount_key(), entry.inode)
}

#[derive(Deserialize, Serialize)]
struct Record {
    version: u8,
    target: JournalPath,
    entry: Entry,
    mode: u32,
    complete: Option<ContentIdentity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    new_len: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    proof: Option<JournalPath>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pages: Option<proof::Pages>,
    #[serde(default)]
    created: Option<SystemTime>,
    #[serde(default)]
    birth: Option<SystemTime>,
}

pub(crate) struct Undo<'a> {
    tx: &'a Transaction<'a>,
    preparation: Preparation,
    record: Record,
    source: &'a File,
}

enum Preparation {
    Clone(tempfile::TempPath, File),
    Inline(Stage),
}

impl<'a> Transaction<'a> {
    pub(crate) fn prepare_undo<'b>(
        &'b self,
        source: &'b File,
        original: &Metadata,
        len: u64,
        encoded: Option<crate::save::Encoded<'_>>,
    ) -> Result<Option<Undo<'b>>, String> {
        if fault(Phase::Backup).is_err() {
            return Ok(None);
        }
        let error = |e| regular::error(self.target, e);
        ensure_directory(&self.root).map_err(error)?;
        let record = Record {
            version: 4,
            target: JournalPath::new(self.target),
            entry: self.entry,
            mode: original.mode() & 0o7777,
            complete: encoded.map(|revision| revision.content),
            new_len: Some(len),
            proof: None,
            pages: None,
            created: Some(SystemTime::now()),
            birth: original.created().ok(),
        };
        let limit = crate::save::INLINE_LIMIT as u64;
        if let Some(encoded) = encoded
            && len <= limit
            && original.len() <= limit
            // Moving between empty and non-empty on FAT/exFAT can change
            // the synthetic file ID. The forward journal knows about it.
            && !(self.volume.synthetic_empty_id && (original.len() == 0 || len == 0))
        {
            fault(Phase::Journal).map_err(error)?;
            let stage = inline::stage(&self.root, source, original, &record, &encoded)?;
            return Ok(Some(Undo {
                tx: self,
                preparation: Preparation::Inline(stage),
                record,
                source,
            }));
        }
        if !self.volume.apfs || fs::metadata(&self.root).map_err(error)?.dev() != original.dev() {
            return Ok(None);
        }
        let parent = File::open(&self.root).map_err(error)?;
        // https://github.com/apple-oss-distributions/xnu/blob/main/bsd/man/man2/clonefile.2
        let clone = tempfile::Builder::new()
            .prefix(&preparation_prefix(BACKUP))
            .make_in(&self.root, |path| {
                rustix::fs::fclonefileat(source, &parent, path, rustix::fs::CloneFlags::NOOWNERCOPY)
                    .map_err(io::Error::from)
            });
        let temporary = match clone {
            Ok(clone) => clone.into_temp_path(),
            // A cross-volume profile, non-COW volume or restricted clone is
            // handled by the staged forward journal, not an unprotected write.
            Err(_) => return Ok(None),
        };
        let Ok(file) = regular::read(&temporary) else {
            return Ok(None);
        };
        #[cfg(test)]
        fault(Phase::ClonePrepared).map_err(error)?;
        Ok(Some(Undo {
            tx: self,
            preparation: Preparation::Clone(temporary, file),
            record,
            source,
        }))
    }
}

impl Undo<'_> {
    pub(crate) fn write(
        mut self,
        target: &File,
        original: &Metadata,
        len: u64,
        encode: impl Fn(&mut (dyn Write + Send)) -> Result<ContentIdentity, String>,
    ) -> Result<Saved, String> {
        let path = self.tx.target;
        let error = |e| regular::error(path, e);
        let inline = matches!(self.preparation, Preparation::Inline(_));
        let journal = self.tx.root.join(if inline {
            inline::name(self.tx.entry)
        } else {
            name(self.tx.entry)
        });
        let mut record = self.record;
        let (mut proof, encoded) = if inline {
            (None, None)
        } else {
            let mut scan = proof::Scan::new(len, original.len()).map_err(error)?;
            // Reuse a bounded medium-size encode in RAM. Large documents
            // still use the streaming encoder and never spool a second
            // copy of the document onto the target's volume.
            let mut bytes = Vec::new();
            let (content, encoded) =
                if len <= 64 * 1024 * 1024 && bytes.try_reserve_exact(len as usize).is_ok() {
                    let content = encode(&mut bytes).map_err(|e| regular::error(path, e))?;
                    scan.write_all(&bytes).map_err(error)?;
                    (content, Some(bytes))
                } else {
                    let content = encode(&mut scan).map_err(|e| regular::error(path, e))?;
                    (content, None)
                };
            if content.bytes != len {
                return Err(regular::error(path, "encoded length changed"));
            }
            // The clone's barrier orders these same-device writes too.
            let (stage, pages) = scan.save(&self.tx.root).map_err(error)?;
            record.complete = Some(content);
            record.pages = Some(pages);
            fault(Phase::Journal).map_err(error)?;
            let Preparation::Clone(_, file) = &self.preparation else {
                unreachable!()
            };
            store(file, &record).map_err(error)?;
            (Some(stage), encoded)
        };
        crate::save::check_target(path, original)?;
        let extend = reserve(
            target,
            original.len(),
            len,
            self.tx.volume.synthetic_empty_id,
        )
        .map_err(error)?;
        check_entry(path, target, self.tx.entry)?;
        let file: &File = match &mut self.preparation {
            Preparation::Clone(temporary, file) => {
                crate::exclusive_rename(temporary, &journal).map_err(error)?;
                temporary.disable_cleanup(true);
                proof.as_mut().expect("clone proof").retain_for_recovery();
                file
            }
            Preparation::Inline(stage) => {
                stage.retain();
                stage.descriptor()
            }
        };
        // The rollback bytes, record and directory entry must precede any pwrite.
        // BARRIERFSYNC orders earlier writes on Apple storage; only the final
        // F_FULLFSYNC requests a device flush. A rollback on another volume
        // needs its own full flush before we touch the target.
        // https://developer.apple.com/videos/play/wwdc2019/419/?time=766
        atomic::sync_parent(&journal).map_err(|e| pending(path, &journal, &e))?;
        let durability = recovery_durability(original.dev(), file.metadata().map_err(error)?.dev());
        atomic::sync(file, durability).map_err(|e| pending(path, &journal, &e))?;
        let comparison = (len >= 32 * 1024 * 1024).then(|| Compare {
            source: self.source,
            original_len: original.len(),
            bytes: Vec::new(),
            different: 0,
        });
        let write = || -> Result<Saved, String> {
            if extend {
                fault(Phase::Extend)
                    .and_then(|()| target.set_len(len))
                    .map_err(error)?;
            }
            check_entry(path, target, record.entry)?;
            let mut output = Positional {
                file: target,
                offset: 0,
                comparison,
                #[cfg(test)]
                page_proof: !inline,
            };
            let content = if let Some(encoded) = encoded {
                for bytes in encoded.chunks(4 * 1024 * 1024) {
                    output.write_all(bytes).map_err(error)?;
                }
                record.complete.expect("buffered encode was hashed")
            } else {
                encode(&mut output).map_err(|e| regular::error(path, e))?
            };
            if content.bytes != len || output.offset != len {
                return Err(regular::error(path, "encoded length changed"));
            }
            fault(Phase::Truncate).map_err(error)?;
            if len < original.len() {
                target.set_len(len).map_err(error)?;
            }
            let warning = preserve_mode(target, path, record.mode);
            if record.complete.as_ref() != Some(&content) {
                return Err(regular::error(path, "encoded revision changed during save"));
            }
            fault(Phase::SyncTarget)
                .and_then(|()| atomic::sync(target, Durability::Full))
                .map_err(error)?;
            #[cfg(test)]
            fault(Phase::AfterSync).map_err(error)?;
            Ok(Saved { content, warning })
        };
        match write() {
            Ok(mut saved) => {
                // Removal need not be durable: the old clone, if resurrected,
                // must still carry the exact checksum of the completed write.
                if let Err(e) = discard(&journal, &record) {
                    saved.warning = Some(pending(path, &journal, &e));
                }
                Ok(saved)
            }
            Err(e) => {
                check_entry(path, target, record.entry).map_err(|e| pending(path, &journal, &e))?;
                let backup = Backup {
                    file,
                    len: original.len(),
                    modified: original.modified().ok(),
                };
                let failed = |restore: &dyn std::fmt::Display| {
                    pending(path, &journal, &format!("{e}; rollback: {restore}"))
                };
                let now = target.metadata().map_err(|err| failed(&err))?;
                let revision = verify(&journal, &record, &backup, inline, self.source, now.len())
                    .map_err(|err| failed(&err))?;
                check_recovery_revision(path, target, &now).map_err(|err| failed(&err))?;
                match revision {
                    proof::Overwrite::Original => (),
                    proof::Overwrite::Interrupted { through } => {
                        rollback(backup, target, Some(self.source), record.mode, through)
                            .map_err(|err| failed(&err))?;
                    }
                    proof::Overwrite::Foreign => {
                        let action = archive(
                            &journal,
                            &record,
                            backup,
                            inline,
                            "file changed during the failed save",
                        )
                        .map_err(|err| failed(&err))?;
                        return Err(format!("{e}; {}", action.message.unwrap_or_default()));
                    }
                }
                discard(&journal, &record).map_err(|err| failed(&err))?;
                Err(format!("{e}; restored the original file"))
            }
        }
    }
}

fn store(file: &File, record: &Record) -> io::Result<()> {
    let bytes = serde_json::to_vec(record)?;
    if bytes.len() as u64 > MAX_RECORD {
        return Err(io::Error::other("save journal is too large"));
    }
    rustix::fs::fsetxattr(file, ATTR, &bytes, rustix::fs::XattrFlags::empty()).map_err(Into::into)
}

struct Positional<'a> {
    file: &'a File,
    offset: u64,
    comparison: Option<Compare<'a>>,
    #[cfg(test)]
    page_proof: bool,
}

#[cfg(test)]
thread_local! {
    pub(super) static PARTIAL_WRITE: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
}

struct Compare<'a> {
    source: &'a File,
    original_len: u64,
    bytes: Vec<u8>,
    different: usize,
}

impl Compare<'_> {
    fn unchanged(&mut self, offset: u64, bytes: &[u8]) -> bool {
        if self.different == 2 || offset.saturating_add(bytes.len() as u64) > self.original_len {
            return false;
        }
        // Read only the not-yet-written extent from the original descriptor.
        // A small edit can keep the rest of the old extents. For a rewrite,
        // stop comparing after two consecutive differences (no read-back pass).
        let mut prefix = [0; 4096];
        let probe = bytes.len().min(prefix.len());
        let same_prefix = self
            .source
            .read_exact_at(&mut prefix[..probe], offset)
            .is_ok()
            && prefix[..probe] == bytes[..probe];
        let unchanged = same_prefix && {
            self.bytes.resize(bytes.len() - probe, 0);
            self.source
                .read_exact_at(&mut self.bytes, offset + probe as u64)
                .is_ok()
                && self.bytes == bytes[probe..]
        };
        if unchanged {
            self.different = 0;
            true
        } else {
            self.different += 1;
            false
        }
    }
}

impl Write for Positional<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        #[cfg(test)]
        fault(Phase::BeforeWrite)?;
        #[cfg(test)]
        if self.offset == 0 && !bytes.is_empty() && has_fault(Phase::Write) {
            let partial = PARTIAL_WRITE.get().unwrap_or_else(|| {
                if self.page_proof {
                    // Simulate a process kill after one complete page.
                    bytes.len().min(proof::PAGE as usize)
                } else {
                    bytes.len().div_ceil(2).min(proof::PAGE as usize)
                }
            });
            let _ = self.file.write_at(&bytes[..partial.min(bytes.len())], 0)?;
            fault(Phase::Write)?;
        }
        let n = if self
            .comparison
            .as_mut()
            .is_some_and(|c| c.unchanged(self.offset, bytes))
        {
            bytes.len()
        } else {
            // An interrupted/short write is retried by Write::write_all. Do
            // not compare a partial write's remainder with the recovery data.
            let n = self.file.write_at(bytes, self.offset)?;
            if n < bytes.len() {
                self.comparison = None;
            }
            n
        };
        self.offset += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

struct Backup<'a> {
    file: &'a File,
    len: u64,
    modified: Option<SystemTime>,
}

fn rollback(
    backup: Backup<'_>,
    target: &File,
    readable: Option<&File>,
    mode: u32,
    changed_until: u64,
) -> io::Result<()> {
    let original_len = backup.len;
    let current_len = target.metadata()?.len();
    let changed_until = changed_until.min(original_len).min(current_len);
    if changed_until == 0 && current_len == original_len {
        return Ok(());
    }
    #[cfg(test)]
    fault(Phase::Rollback)?;
    // Repair EOF before changing the common prefix. A killed rollback may
    // leave a truncated growth, or part of the original tail after a shrink.
    // Both are recognizable against the saved versions on the next launch.
    if current_len > original_len {
        target.set_len(original_len)?;
    } else if current_len < original_len {
        let mut offset = current_len;
        let mut bytes = vec![0; (original_len - offset).min(1024 * 1024) as usize];
        while offset < original_len {
            let n = (original_len - offset).min(bytes.len() as u64) as usize;
            backup.file.read_exact_at(&mut bytes[..n], offset)?;
            target.write_all_at(&bytes[..n], offset)?;
            offset += n as u64;
            #[cfg(test)]
            fault(Phase::RollbackTail)?;
        }
    }
    // Read from the already-locked inode. Opening a separate fd is only for
    // the comparison; a write-only document can still be restored.
    let mut old = vec![0; changed_until.min(1024 * 1024) as usize];
    let mut new = vec![0; old.len()];
    let mut end = changed_until;
    while end != 0 {
        // Keep reverse writes aligned with the page proofs even when the
        // document, or the last partial pwrite, ended between page boundaries.
        let offset = (end - 1) / (1024 * 1024) * (1024 * 1024);
        let n = (end - offset) as usize;
        backup.file.read_exact_at(&mut old[..n], offset)?;
        if readable.is_some_and(|file| file.read_exact_at(&mut new[..n], offset).is_ok()) {
            // Don't turn unchanged, shared APFS blocks into new allocations
            // just to discover that the volume is still out of space.
            // Restore from the end so another interruption between writes
            // still leaves a matching new prefix followed by the old suffix.
            for (i, (before, after)) in old[..n]
                .chunks(proof::PAGE as usize)
                .zip(new[..n].chunks(proof::PAGE as usize))
                .enumerate()
                .rev()
            {
                if before != after {
                    target.write_all_at(before, offset + i as u64 * proof::PAGE)?;
                    #[cfg(test)]
                    fault(Phase::RollbackWrite)?;
                }
            }
        } else {
            target.write_all_at(&old[..n], offset)?;
        }
        end = offset;
    }
    mode_after_write(target, mode)?;
    if let Some(time) = backup.modified {
        let _ = target.set_modified(time);
    }
    atomic::sync(target, Durability::Full)
}

fn read_clone_record(path: &Path, file: &File) -> Result<Record, String> {
    let size = rustix::fs::fgetxattr_query(file, ATTR).map_err(|e| regular::error(path, e))?;
    if size as u64 > MAX_RECORD {
        return Err(regular::error(path, "invalid rollback record size"));
    }
    let mut bytes = vec![0; size];
    let actual =
        rustix::fs::fgetxattr(file, ATTR, &mut bytes[..]).map_err(|e| regular::error(path, e))?;
    serde_json::from_slice(&bytes[..actual]).map_err(|e| regular::error(path, e))
}

pub(super) fn refers_to(path: &Path, artifact: &Path) -> Result<bool, String> {
    let file = regular::read(path)?;
    let record = read_clone_record(path, &file)?;
    Ok(record.proof.is_some_and(|proof| proof.get() == artifact)
        || record.pages.is_some_and(|pages| pages.path() == artifact))
}

fn discard(journal: &Path, record: &Record) -> io::Result<()> {
    fs::remove_file(journal)?;
    for path in [
        record.proof.as_ref().map(JournalPath::get),
        record.pages.as_ref().map(proof::Pages::path),
    ]
    .into_iter()
    .flatten()
    {
        match fs::remove_file(path) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => (),
            result => result?,
        }
        stages::remove_companion(path).map_err(io::Error::other)?;
        stages::forget(atomic::parent(journal), path)?;
    }
    Ok(())
}

pub(super) fn recover(path: &Path, held: Option<&File>) -> Result<RecoveryAction, String> {
    let error = |e| regular::error(path, e);
    let file = regular::read(path)?;
    let metadata = file.metadata().map_err(error)?;
    let inode = Entry::of(&metadata);
    let inline = path.extension().is_some_and(|ext| ext == "save-small");
    let (record, backup) = if inline {
        match inline::read(path, &file)? {
            inline::Found::Record(record, backup) => (*record, backup),
            inline::Found::Unpublished(message) => {
                return Ok(RecoveryAction {
                    message: Some(message),
                    verified: false,
                });
            }
        }
    } else {
        (
            read_clone_record(path, &file)?,
            Backup {
                file: &file,
                len: metadata.len(),
                modified: metadata.modified().ok(),
            },
        )
    };
    let target = record.target.get();
    let name = if inline {
        inline::name(record.entry)
    } else {
        name(record.entry)
    };
    if !matches!(record.version, 1..=4)
        || !target.is_absolute()
        || path.file_name().is_none_or(|n| n != name.as_str())
        || (record.version >= 3
            && record
                .new_len
                .zip(record.complete)
                .is_none_or(|(len, content)| len != content.bytes))
        || record.proof.as_ref().is_some_and(|proof| {
            inline
                || proof.get().parent() != Some(atomic::parent(path))
                || !staged_name(proof.get(), STAGE)
        })
        || (!inline && record.version == 3 && record.proof.is_none())
        || (record.version == 4 && !inline && record.pages.is_none())
        || record.pages.as_ref().is_some_and(|pages| {
            inline
                || record.version != 4
                || record.proof.is_some()
                || !pages.valid_in(atomic::parent(path))
        })
    {
        return Err(regular::error(
            path,
            "invalid rollback record; clone retained",
        ));
    }
    let owned;
    let _lock;
    let destination = if let Some(held) = held {
        held
    } else {
        // Opening for write may decompress a foreign file. Classify the
        // existing bytes under a read lock before opening a writable handle.
        owned = match regular::read(target) {
            Ok(file) => file,
            Err(_) if fs::symlink_metadata(target).is_ok_and(|m| !m.is_file()) => {
                return archive(path, &record, backup, inline, "file identity changed");
            }
            Err(_)
                if fs::symlink_metadata(target)
                    .is_err_and(|e| e.kind() == io::ErrorKind::NotFound) =>
            {
                return archive(path, &record, backup, inline, "target no longer exists");
            }
            Err(e) => return Err(pending(target, path, &e)),
        };
        _lock =
            lock(&owned, target, atomic::parent(path)).map_err(|e| pending(target, path, &e))?;
        &owned
    };
    check_entry(path, &file, inode)?;
    let now = destination.metadata().map_err(error)?;
    let mounted = volume::inspect(destination);
    let entry = Entry::on_volume(&now, &mounted);
    check_entry(target, destination, entry).map_err(|e| pending(target, path, &e))?;
    let same_birth = record.birth.is_none_or(|b| now.created().ok() == Some(b));
    let same_file = same_birth && record.entry.same_file(entry);
    let synthetic = inline
        && mounted.synthetic_empty_id
        && record.birth.is_some()
        && same_birth
        && record.entry.same_volume(entry);
    if !same_file && !synthetic {
        return archive(path, &record, backup, inline, "file identity changed");
    }
    let readable = regular::read(target).map_err(|e| pending(target, path, &e))?;
    check_entry(target, &readable, entry).map_err(|e| pending(target, path, &e))?;
    let already_complete = match record.complete {
        Some(content) if now.len() == content.bytes => {
            proof::checksum(&readable, now.len()).map_err(error)? == content
        }
        _ => false,
    };
    if already_complete {
        // The process may have died just before the final flush.
        check_recovery_revision(target, destination, &now)?;
        atomic::sync(destination, Durability::Full).map_err(error)?;
        discard(path, &record).map_err(error)?;
        return Ok(RecoveryAction {
            message: same_file
                .then(|| format!("Completed save already intact: {}", target.display())),
            verified: true,
        });
    }
    if !same_file {
        return archive(path, &record, backup, inline, "file identity changed");
    }
    let revision = verify(path, &record, &backup, inline, &readable, now.len())
        .unwrap_or(proof::Overwrite::Foreign);
    check_recovery_revision(target, destination, &now)?;
    let message = match revision {
        proof::Overwrite::Original => format!("Original already intact: {}", target.display()),
        proof::Overwrite::Interrupted { through } => {
            let fresh;
            let write = if let Some(held) = held {
                held
            } else {
                fresh = regular::writable(target).map_err(|e| pending(target, path, &e))?;
                check_recovery_revision(target, &fresh, &now)?;
                &fresh
            };
            rollback(backup, write, Some(&readable), record.mode, through)
                .map_err(|e| pending(target, path, &e))?;
            format!(
                "Restored original after interrupted save: {}",
                target.display()
            )
        }
        proof::Overwrite::Foreign => {
            return archive(
                path,
                &record,
                backup,
                inline,
                "file changed since the interrupted save",
            );
        }
    };
    discard(path, &record).map_err(error)?;
    Ok(RecoveryAction {
        message: Some(message),
        verified: true,
    })
}

fn verify(
    path: &Path,
    record: &Record,
    backup: &Backup<'_>,
    inline: bool,
    target: &File,
    actual_len: u64,
) -> Result<proof::Overwrite, String> {
    let error = |e| regular::error(path, e);
    let len = record.new_len.unwrap_or_default();
    let table;
    let postimage;
    let intended = if inline && record.version >= 3 {
        Some(proof::Revision::file(backup.file, backup.len, len))
    } else if let Some(pages) = &record.pages {
        table = pages.read(len, backup.len).map_err(error)?;
        Some(proof::Revision::pages(&table, len, backup.len))
    } else {
        postimage = verified_postimage(path, record)?;
        postimage
            .as_ref()
            .map(|image| proof::Revision::file(image, 0, len))
    };
    let old = proof::Revision::file(backup.file, 0, backup.len);
    proof::verify(target, actual_len, old, intended).map_err(error)
}

fn verified_postimage(path: &Path, record: &Record) -> Result<Option<File>, String> {
    let Some(stage) = &record.proof else {
        return Ok(None);
    };
    let image = stages::open_owned(stage.get())?.map(|(file, _)| file);
    if let Some(image) = &image {
        let len = record.new_len.unwrap_or_default();
        let error = |e| regular::error(path, e);
        if image.metadata().map_err(error)?.len() != len
            || Some(proof::checksum(image, len).map_err(error)?) != record.complete
        {
            return Err(pending(
                record.target.get(),
                path,
                &"new revision proof changed; no data was overwritten",
            ));
        }
    }
    Ok(image)
}

fn archive(
    path: &Path,
    record: &Record,
    backup: Backup<'_>,
    inline: bool,
    reason: &str,
) -> Result<RecoveryAction, String> {
    use retained::{Image, archive_image};
    let target = record.target.get();
    let original = Image::file(backup.file, 0, backup.len);
    let original = archive_image(
        path,
        target,
        if inline {
            original
        } else {
            original.link(path)
        },
        "original",
    )?;
    let mut message = format!(
        "{}: {reason}; no data was overwritten. Rollback copy kept at {}",
        target.display(),
        original.display()
    );
    let len = record.new_len.unwrap_or_default();
    let staged = verified_postimage(path, record)?;
    let new = if inline && record.version >= 3 {
        Some(Image::file(backup.file, backup.len, len))
    } else {
        staged.as_ref().map(|image| Image::file(image, 0, len))
    };
    if let Some(new) = new {
        let saved = archive_image(path, target, new, "new")?;
        message.push_str(&format!("; new revision kept at {}", saved.display()));
    }
    // Once published, archives survive a crash between the two publications
    // or before cleanup. Retrying uses the same names and compares the bytes.
    discard(path, record).map_err(|e| pending(target, path, &e))?;
    Ok(RecoveryAction {
        message: Some(message),
        verified: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::save::Encoded;

    #[test]
    fn legacy_clone_archives_both_versions_when_a_later_edit_is_present() {
        let profile = tempfile::tempdir().unwrap();
        let target = profile.path().canonicalize().unwrap().join("target");
        let root = profile.path().join("Recovery");
        ensure_directory(&root).unwrap();
        fs::write(&target, "foreign edit to the same inode").unwrap();
        let source = regular::read(&target).unwrap();
        let metadata = source.metadata().unwrap();
        let entry = Entry::on_volume(&metadata, &volume::inspect(&source));
        let stage = root.join(".velocity-save-data-legacy");
        let new = b"complete new bytes from the editor";
        fs::write(&stage, new).unwrap();
        let clone = root.join(name(entry));
        fs::write(&clone, "whole original revision").unwrap();
        let record = Record {
            version: 3,
            target: JournalPath::new(&target),
            entry,
            mode: metadata.mode() & 0o7777,
            complete: Some(ContentIdentity {
                bytes: new.len() as u64,
                hash: *blake3::hash(new).as_bytes(),
            }),
            new_len: Some(new.len() as u64),
            proof: Some(JournalPath::new(&stage)),
            pages: None,
            created: None,
            birth: metadata.created().ok(),
        };
        store(&regular::read(&clone).unwrap(), &record).unwrap();
        let notes = recover_saves(Some(profile.path()));
        assert_eq!(notes.len(), 1, "{notes:?}");
        assert!(
            notes[0].contains("Rollback copy kept at") && notes[0].contains("new revision kept at")
        );
        let mut contents: Vec<_> = root
            .read_dir()
            .unwrap()
            .flatten()
            .map(|e| {
                assert!(staged_name(&e.path(), retained::PREFIX));
                fs::read_to_string(e.path()).unwrap()
            })
            .collect();
        contents.sort();
        assert_eq!(
            contents,
            [
                "complete new bytes from the editor",
                "whole original revision"
            ]
        );
        assert_eq!(
            fs::read_to_string(&target).unwrap(),
            "foreign edit to the same inode"
        );
        assert!(recover_saves(Some(profile.path())).is_empty());
    }

    #[test]
    fn legacy_inline_on_fat_archives_changed_ids_and_recognizes_completed_truncate() {
        for (new, external) in [("editor revision", Some("foreign rewrite")), ("", None)] {
            let profile = tempfile::tempdir().unwrap();
            let target = profile.path().canonicalize().unwrap().join("target");
            let root = profile.path().join("Recovery");
            ensure_directory(&root).unwrap();
            let original = "o".repeat(8192);
            fs::write(&target, &original).unwrap();
            let source = regular::read(&target).unwrap();
            let metadata = source.metadata().unwrap();
            let mut entry = Entry::on_volume(&metadata, &volume::inspect(&source));
            entry.inode ^= 1; // Synthetic ID from before FAT rewrote the cluster chain.
            let mut digest = crate::identity::Digest::default();
            digest.update(new.as_bytes());
            let content = digest.identity();
            let record = Record {
                version: 3,
                target: JournalPath::new(&target),
                entry,
                mode: metadata.mode() & 0o7777,
                complete: Some(content),
                new_len: Some(content.bytes),
                proof: None,
                pages: None,
                created: Some(SystemTime::UNIX_EPOCH), // Age must never retire user bytes.
                birth: metadata.created().ok(),
            };
            let mut image = inline::stage(
                &root,
                &source,
                &metadata,
                &record,
                &Encoded {
                    bytes: new.as_bytes(),
                    content,
                },
            )
            .unwrap();
            image.retain();
            drop(image);
            let foreign = external.unwrap_or(new);
            fs::write(&target, foreign).unwrap();
            volume::FAT_PROBE.set(true);
            let notices = recover_saves(Some(profile.path()));
            volume::FAT_PROBE.set(false);
            let mut copies: Vec<_> = root
                .read_dir()
                .unwrap()
                .flatten()
                .map(|entry| {
                    assert!(staged_name(&entry.path(), retained::PREFIX));
                    fs::read_to_string(entry.path()).unwrap()
                })
                .collect();
            copies.sort();
            assert_eq!(fs::read_to_string(&target).unwrap(), foreign);
            if external.is_some() {
                assert_eq!(notices.len(), 1, "{notices:?}");
                assert!(notices[0].contains("file identity changed"), "{notices:?}");
                assert_eq!(copies, [new, &original]);
            } else {
                assert!(notices.is_empty(), "{notices:?}");
                assert!(copies.is_empty(), "{copies:?}");
            }
            assert!(recover_saves(Some(profile.path())).is_empty());
        }
    }

    #[test]
    fn saving_empty_on_fat_uses_a_forward_record() {
        use crate::save_journal::tests::{io_failure, save, use_faults};
        let profile = tempfile::tempdir().unwrap();
        let target = profile.path().join("target");
        fs::write(&target, "original").unwrap();
        volume::FAT_PROBE.set(true);
        let error = use_faults(
            &[
                (Phase::AfterSync, io_failure),
                (Phase::Backup, || Err(rustix::io::Errno::NOTSUP.into())),
            ],
            || save(&target, profile.path(), "").unwrap_err(),
        );
        assert!(error.contains("complete staged content"));
        let entries: Vec<_> = profile
            .path()
            .join("Recovery")
            .read_dir()
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .collect();
        assert!(
            entries
                .iter()
                .any(|p| p.extension().is_some_and(|e| e == "save-journal"))
        );
        assert!(
            entries
                .iter()
                .all(|p| p.extension().is_none_or(|e| e != "save-small"))
        );
        assert!(recover_saves(Some(profile.path()))[0].contains("already intact"));
        volume::FAT_PROBE.set(false);
        assert!(fs::read(&target).unwrap().is_empty());
    }
}
