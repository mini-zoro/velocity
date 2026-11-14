use crate::{FileAnalysis, Index};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs::{File, OpenOptions},
    io::{self, Read, Seek, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Stamp {
    pub(crate) len: u64,
    pub(crate) modified: u128,
}
impl Stamp {
    pub(crate) fn read(path: &Path) -> io::Result<Self> {
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
#[derive(Debug, Serialize, Deserialize)]
struct Record {
    path: PathBuf,
    stamp: Option<Stamp>,
    analysis: Option<FileAnalysis>,
}
#[derive(Debug)]
pub(crate) struct Journal {
    file: File,
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
        let mut file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(path)?;
        let mut stamps = HashMap::new();
        let mut valid = 8;
        let mut magic = [0; 8];
        if file.read_exact(&mut magic).is_err() || &magic != b"VELIDX06" {
            file.set_len(0)?;
            file.rewind()?;
            file.write_all(b"VELIDX06")?;
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
                if let (Some(stamp), Some(analysis)) = (record.stamp, record.analysis) {
                    index.replace_file(record.path.clone(), analysis);
                    stamps.insert(record.path, stamp);
                } else {
                    index.remove_file(&record.path);
                    stamps.remove(&record.path);
                }
                valid += 36 + len as u64;
            }
        }
        // Discard only the incomplete/corrupt tail, so future appends remain replayable.
        file.set_len(valid)?;
        file.seek(io::SeekFrom::End(0))?;
        Ok((Self { file, failed: None }, stamps))
    }
    pub(crate) fn append(
        &mut self,
        path: PathBuf,
        stamp: Option<Stamp>,
        analysis: Option<FileAnalysis>,
    ) -> io::Result<()> {
        if let Some(error) = &self.failed {
            return Err(io::Error::other(error.clone()));
        }
        let bytes = bincode::serialize(&Record {
            path,
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
        }
        written
    }
    pub(crate) fn flush(&mut self) -> io::Result<()> {
        if let Some(error) = &self.failed {
            return Err(io::Error::other(error.clone()));
        }
        self.file.sync_data()
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
}
