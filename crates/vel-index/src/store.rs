use crate::{FileAnalysis, Index};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions},
    io::{self, Read, Seek, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};
const MAGIC: &[u8] = b"VELIDX06";
const HEADER: u64 = 8;
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Stamp {
    pub(crate) len: u64,
    pub(crate) modified: u128,
}
impl Stamp {
    pub(crate) fn read(path: &Path) -> io::Result<Self> {
        #[cfg(test)]
        STAT_READS.with(|reads| reads.set(reads.get() + 1));
        let metadata = path.metadata()?;
        Ok(Self {
            len: metadata.len(),
            modified: metadata
                .modified()?
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos(),
        })
    }
}
#[cfg(test)]
thread_local! {
    pub(crate) static STAT_READS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}
#[derive(Debug, Serialize, Deserialize)]
struct Record {
    path: PathBuf,
    stamp: Option<Stamp>,
    analysis: Option<FileAnalysis>,
}
#[derive(Debug)]
pub(crate) struct Journal {
    file: File,
    path: PathBuf,
    #[cfg(unix)]
    lock: Option<File>,
    end: u64,
    live: u64,
    frames: HashMap<PathBuf, (u64, u64)>,
    failed: Option<String>,
}
impl Journal {
    pub(crate) fn open(
        path: &Path,
        index: &mut Index,
        cancelled: &AtomicBool,
    ) -> io::Result<(Self, HashMap<PathBuf, Stamp>)> {
        if cancelled.load(Ordering::Acquire) {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "index cancelled",
            ));
        }
        #[cfg(unix)]
        let lock = journal_lock(path)?;
        #[cfg(unix)]
        let writable = lock.is_some();
        #[cfg(not(unix))]
        let writable = true;
        let mut file = OpenOptions::new()
            .create(writable)
            .read(true)
            .write(writable)
            .truncate(false)
            .open(path)
            .or_else(|error| {
                if cfg!(unix) && !writable && error.kind() == io::ErrorKind::NotFound {
                    // A peer holds the lock but has not opened the journal yet.
                    File::open("/dev/null")
                } else {
                    Err(error)
                }
            })?;
        let mut stamps = HashMap::new();
        let mut frames = HashMap::new();
        let mut valid = HEADER;
        let mut live = HEADER;
        let mut magic = [0; 8];
        if file.read_exact(&mut magic).is_err() || magic != MAGIC {
            if writable {
                file.set_len(0)?;
                file.rewind()?;
                file.write_all(MAGIC)?;
            }
        } else {
            loop {
                if cancelled.load(Ordering::Acquire) {
                    return Err(io::Error::new(
                        io::ErrorKind::Interrupted,
                        "index cancelled",
                    ));
                }
                let mut len = [0; 4];
                if file.read_exact(&mut len).is_err() {
                    break;
                }
                let len = u32::from_le_bytes(len) as usize;
                if len > 64 * 1024 * 1024 {
                    break;
                }
                let mut checksum = [0; 32];
                if file.read_exact(&mut checksum).is_err() {
                    break;
                }
                let mut bytes = vec![0; len];
                if file.read_exact(&mut bytes).is_err()
                    || blake3::hash(&bytes).as_bytes() != &checksum
                {
                    break;
                }
                let record: Record = match bincode::deserialize(&bytes) {
                    Ok(record) => record,
                    Err(_) => break,
                };
                let size = 36 + len as u64;
                if let Some((_, size)) = frames.remove(&record.path) {
                    live -= size;
                }
                if let (Some(stamp), Some(analysis)) = (record.stamp, record.analysis) {
                    index.replace_file(record.path.clone(), analysis);
                    frames.insert(record.path.clone(), (valid, size));
                    live += size;
                    stamps.insert(record.path, stamp);
                } else {
                    index.remove_file(&record.path);
                    stamps.remove(&record.path);
                }
                valid += size;
            }
        }
        // Discard only the incomplete/corrupt tail, so future appends remain replayable.
        if writable {
            file.set_len(valid)?;
            file.seek(io::SeekFrom::End(0))?;
        }
        Ok((
            Self {
                file,
                path: path.to_owned(),
                #[cfg(unix)]
                lock,
                end: valid,
                live,
                frames,
                failed: None,
            },
            stamps,
        ))
    }
    pub(crate) fn append(
        &mut self,
        path: PathBuf,
        stamp: Option<Stamp>,
        analysis: Option<FileAnalysis>,
    ) -> io::Result<()> {
        #[cfg(unix)]
        if self.lock.is_none() {
            return Ok(());
        }
        if let Some(error) = &self.failed {
            return Err(io::Error::other(error.clone()));
        }
        let bytes = bincode::serialize(&Record {
            path: path.clone(),
            stamp,
            analysis,
        })
        .map_err(io::Error::other)?;
        let len: u32 = bytes.len().try_into().map_err(io::Error::other)?;
        if bytes.len() > 64 * 1024 * 1024 {
            return Err(io::Error::other("index cache record exceeds 64 MiB"));
        }
        let written = (|| {
            self.file.write_all(&len.to_le_bytes())?;
            self.file.write_all(blake3::hash(&bytes).as_bytes())?;
            self.file.write_all(&bytes)
        })();
        if let Err(error) = &written {
            self.failed = Some(error.to_string());
        } else {
            let size = 36 + u64::from(len);
            if let Some((_, old)) = self.frames.remove(&path) {
                self.live -= old;
            }
            if stamp.is_some() {
                self.frames.insert(path, (self.end, size));
                self.live += size;
            }
            self.end += size;
        }
        written
    }
    pub(crate) fn compact(&mut self, cancelled: &AtomicBool) -> io::Result<()> {
        #[cfg(unix)]
        if self.lock.is_none() {
            return Ok(());
        }
        if self.end <= self.live.saturating_mul(2) || self.failed.is_some() {
            return Ok(());
        }
        static SERIAL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let serial = SERIAL.fetch_add(1, Ordering::Relaxed);
        let tmp = self
            .path
            .with_extension(format!("tmp-{}-{serial}", std::process::id()));
        let mut fresh = OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(&tmp)?;
        let compacted = (|| {
            fresh.write_all(MAGIC)?;
            let mut frames = HashMap::with_capacity(self.frames.len());
            let mut end = HEADER;
            let mut ordered: Vec<_> = self.frames.iter().collect();
            ordered.sort_unstable_by_key(|(_, (offset, _))| *offset);
            for (path, &(offset, size)) in ordered {
                if cancelled.load(Ordering::Acquire) {
                    return Err(io::ErrorKind::Interrupted.into());
                }
                self.file.seek(io::SeekFrom::Start(offset))?;
                if io::copy(&mut (&mut self.file).take(size), &mut fresh)? != size {
                    return Err(io::ErrorKind::UnexpectedEof.into());
                }
                frames.insert(path.clone(), (end, size));
                end += size;
            }
            fresh.sync_data()?;
            fs::rename(&tmp, &self.path)?;
            Ok((frames, end))
        })();
        match compacted {
            Ok((frames, end)) => {
                self.frames = frames;
                self.end = end;
                self.file = fresh;
                Ok(())
            }
            Err(error) => {
                let _ = fs::remove_file(tmp);
                self.file.seek(io::SeekFrom::End(0))?;
                Err(error)
            }
        }
    }
    pub(crate) fn flush(&mut self) -> io::Result<()> {
        #[cfg(unix)]
        if self.lock.is_none() {
            return Ok(());
        }
        if let Some(error) = &self.failed {
            return Err(io::Error::other(error.clone()));
        }
        self.file.sync_data()
    }
}

#[cfg(unix)]
fn journal_lock(path: &Path) -> io::Result<Option<File>> {
    use std::os::{fd::AsRawFd, unix::fs::OpenOptionsExt};
    let dir = path.parent().expect("journal directory").join(".locks");
    // The coordinator has already created the journal directory. Do not recreate
    // it if a concurrent process-exit cleanup has since removed a temporary cache.
    if let Err(error) = fs::create_dir(&dir)
        && error.kind() != io::ErrorKind::AlreadyExists
    {
        return Err(error);
    }
    let lock = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(dir.join(path.file_name().expect("journal filename")))?;
    // Keep the lock on a separate file so compaction by rename cannot release it.
    // SAFETY: lock owns a live descriptor.
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
        return Ok(Some(lock));
    }
    let error = io::Error::last_os_error();
    if error.kind() == io::ErrorKind::WouldBlock {
        Ok(None)
    } else {
        Err(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cancellation_does_not_truncate_a_cache_pending_replay() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("cache");
        let bytes = b"VELIDX06partial tail";
        std::fs::write(&path, bytes).unwrap();
        let mut index = Index::new();
        let error = Journal::open(&path, &mut index, &AtomicBool::new(true)).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Interrupted);
        assert_eq!(std::fs::read(path).unwrap(), bytes);
    }
    #[test]
    fn journal_remains_within_twice_live_size_after_200_reindexes() {
        let dir = tempfile::tempdir().unwrap();
        let journal_path = dir.path().join("fixture.v6");
        let source = dir.path().join("file.rs");
        let mut index = Index::new();
        let stop = AtomicBool::new(false);
        let (mut journal, _) = Journal::open(&journal_path, &mut index, &stop).unwrap();
        for modified in 0..200 {
            journal
                .append(
                    source.clone(),
                    Some(Stamp { modified, len: 10 }),
                    Some(FileAnalysis::default()),
                )
                .unwrap();
            journal.compact(&stop).unwrap();
        }
        assert!(journal_path.metadata().unwrap().len() <= journal.live * 2);
        drop(journal);
        let (_, stamps) = Journal::open(&journal_path, &mut index, &stop).unwrap();
        assert_eq!(stamps[&source].modified, 199);
    }
}
