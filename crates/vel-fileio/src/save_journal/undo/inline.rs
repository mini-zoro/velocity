use super::*;

const MAGIC: &[u8; 8] = b"VELUNDO\0";
const FOOTER: usize = 4 + MAGIC.len();

pub(in crate::save_journal) fn name(entry: Entry) -> String {
    format!("{}-{:016x}.save-small", entry.mount_key(), entry.inode)
}

#[derive(Deserialize, Serialize)]
struct InlineRecord<R> {
    record: R,
    original: ContentIdentity,
    modified: Option<SystemTime>,
}

pub(super) fn stage(
    root: &Path,
    source: &File,
    original: &Metadata,
    record: &Record,
    revision: &crate::save::Encoded<'_>,
) -> Result<Stage, String> {
    let target = record.target.get();
    let error = |e| regular::error(target, e);
    let path = root.join(name(record.entry));
    let old_len = original.len() as usize;
    let mut bytes = Vec::with_capacity(old_len + revision.bytes.len() + 512);
    bytes.resize(old_len, 0);
    source.read_exact_at(&mut bytes, 0).map_err(error)?;
    let mut digest = crate::identity::Digest::default();
    digest.update(&bytes);
    let header = serde_json::to_vec(&InlineRecord {
        record,
        original: digest.identity(),
        modified: original.modified().ok(),
    })
    .map_err(|e| regular::error(target, e))?;
    if header.len() as u64 > MAX_RECORD {
        return Err(regular::error(target, "save journal is too large"));
    }
    // Store a footer after the original bytes so the rollback writer can
    // read the same extent from either an inline image or a clone.
    bytes.extend_from_slice(revision.bytes);
    bytes.extend_from_slice(&header);
    bytes.extend_from_slice(&(header.len() as u32).to_le_bytes());
    bytes.extend_from_slice(MAGIC);

    let mut stage = Stage::create_at(&path, None).map_err(error)?;
    let file = stage.descriptor();
    wait_for_lock(file, || file.try_lock().map_err(Into::into)).map_err(error)?;
    // A concurrent recovery may have seen an unused, still-empty slot.
    // Check the name under the slot's own lock before writing anything.
    check_entry(&path, file, Entry::of(&file.metadata().map_err(error)?))?;
    #[cfg(test)]
    fault(Phase::SmallCreated).map_err(error)?;
    #[cfg(test)]
    if has_fault(Phase::SmallPreparing) {
        file.write_all_at(&bytes[..bytes.len() / 2], 0)
            .map_err(error)?;
        fault(Phase::SmallPreparing).map_err(error)?;
    }
    stage.file().write_all(&bytes).map_err(error)?;
    Ok(stage)
}

pub(super) enum Found<'a> {
    Record(Box<Record>, Backup<'a>),
    Unpublished(String),
}

pub(super) fn read<'a>(path: &Path, file: &'a File) -> Result<Found<'a>, String> {
    let error = |e| regular::error(path, e);
    file.try_lock().map_err(|e| regular::error(path, e))?;
    let load = || {
        let metadata = file.metadata().map_err(error)?;
        check_entry(path, file, Entry::of(&metadata))?;
        if metadata.len() == 0 {
            // No write can have started before the complete rollback was
            // synced. Locking also protects a live writer's preparation.
            fs::remove_file(path).map_err(error)?;
            return Ok(Found::Unpublished(format!(
                "Removed unused save rollback entry: {}",
                path.display()
            )));
        }
        let invalid = || regular::error(path, "invalid small-save rollback; retained");
        let unpublished = || {
            // We never start overwriting the original until this record and
            // its footer are ordered on disk. Keep a torn preparation out
            // of the lookup slot without throwing away its bytes.
            let saved = path.with_file_name(format!(
                "{}-{:016x}.incomplete-rollback",
                path.file_stem().unwrap_or_default().to_string_lossy(),
                metadata.ino(),
            ));
            crate::exclusive_rename(path, &saved).map_err(error)?;
            Ok(Found::Unpublished(format!(
                "Incomplete save rollback was not replayed; kept at {}",
                saved.display()
            )))
        };
        let Some(footer) = metadata.len().checked_sub(FOOTER as u64) else {
            return unpublished();
        };
        let mut tail = [0; FOOTER];
        file.read_exact_at(&mut tail, footer).map_err(error)?;
        let count = u32::from_le_bytes(tail[..4].try_into().unwrap()) as u64;
        if &tail[4..] != MAGIC {
            return unpublished();
        }
        if count > MAX_RECORD {
            return Err(invalid());
        }
        let header_at = footer
            .checked_sub(count)
            .filter(|n| *n <= 2 * crate::save::INLINE_LIMIT as u64)
            .ok_or_else(invalid)?;
        let mut bytes = vec![0; count as usize];
        file.read_exact_at(&mut bytes, header_at).map_err(error)?;
        let header: InlineRecord<Record> = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
        let original_len = header.original.bytes;
        let new_len = header.record.new_len.unwrap_or(0);
        let limit = crate::save::INLINE_LIMIT as u64;
        if header.record.complete.is_none()
            || original_len > limit
            || new_len > limit
            || original_len + new_len != header_at
            || proof::checksum(file, original_len).map_err(error)? != header.original
        {
            return Err(invalid());
        }
        if header.record.version >= 3
            && Some(proof::checksum_range(file, original_len, new_len).map_err(error)?)
                != header.record.complete
        {
            return Err(invalid());
        }
        Ok(Found::Record(
            Box::new(header.record),
            Backup {
                file,
                len: original_len,
                modified: header.modified,
            },
        ))
    };
    let result = load();
    // Never hold the rollback lock while looking for the document lock.
    // Saves create the rollback while already holding the document lock.
    file.unlock().map_err(error)?;
    result
}
