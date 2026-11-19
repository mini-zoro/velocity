//! Shared document-file boundary. Never open a pipe/device as an editor document.
use std::{
    fs::{File, Metadata, OpenOptions},
    io,
    path::Path,
};

/// Adds a document path once, including when an inner loader already supplied it.
pub fn error(path: &Path, error: impl std::fmt::Display) -> String {
    let prefix = format!("{}: ", path.display());
    let message = error.to_string();
    let mut detail = message.as_str();
    while let Some(rest) = detail.strip_prefix(&prefix) {
        detail = rest;
    }
    // An inner loader may already name the canonical target of an alias. Keep
    // that provenance without doing filesystem IO while formatting the error.
    if detail
        .split_once(": ")
        .is_some_and(|(source, _)| Path::new(source).is_absolute())
    {
        return detail.to_owned();
    }
    format!("{prefix}{detail}")
}
/// Follows symlinks and accepts regular files; a new write target may be absent.
pub fn metadata(path: &Path, allow_missing: bool) -> Result<Option<Metadata>, String> {
    match std::fs::metadata(path) {
        Ok(m) if m.is_file() => Ok(Some(m)),
        Ok(_) => Err(error(path, "only regular files can be used as documents")),
        Err(e) if allow_missing && e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(error(path, e)),
    }
}
/// Descriptor stamp for the check/open race only; document revisions use ContentIdentity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct DescriptorStamp {
    bytes: u64,
    modified: Option<std::time::SystemTime>,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
}
impl DescriptorStamp {
    fn of(m: &Metadata) -> Self {
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt;
        Self {
            bytes: m.len(),
            modified: m.modified().ok(),
            #[cfg(unix)]
            device: m.dev(),
            #[cfg(unix)]
            inode: m.ino(),
        }
    }
}
#[cfg(test)]
thread_local! { static BEFORE_OPEN: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) }; }
fn open(path: &Path, write: bool) -> Result<File, String> {
    let expected = metadata(path, write)?.as_ref().map(DescriptorStamp::of);
    #[cfg(test)]
    if let Some(swap) = BEFORE_OPEN.with(|hook| hook.take()) {
        swap();
    }
    let mut options = OpenOptions::new();
    options
        .read(!write)
        .write(write)
        .create(write)
        .create_new(write && expected.is_none());
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // O_NONBLOCK closes the check/open FIFO race. File::metadata uses fstat;
        // clear the flag only after verifying the opened descriptor is regular.
        // https://pubs.opengroup.org/onlinepubs/9799919799/functions/open.html
        // https://pubs.opengroup.org/onlinepubs/9799919799/functions/fstat.html
        // macOS semantics: https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/open.2.html
        options
            .custom_flags(rustix::fs::OFlags::NONBLOCK.bits() as i32)
            .mode(0o666);
    }
    let file = options.open(path).map_err(|e| error(path, e))?;
    let opened = file.metadata().map_err(|e| error(path, e))?;
    if !opened.is_file() {
        return Err(error(path, "only regular files can be used as documents"));
    }
    if write && expected.is_some_and(|expected| expected != DescriptorStamp::of(&opened)) {
        return Err(error(
            path,
            "file changed while opening for save; retry after reviewing it",
        ));
    }
    #[cfg(unix)]
    {
        let flags = rustix::fs::fcntl_getfl(&file).map_err(|e| error(path, e))?;
        rustix::fs::fcntl_setfl(&file, flags & !rustix::fs::OFlags::NONBLOCK)
            .map_err(|e| error(path, e))?;
    }
    Ok(file)
}
/// Opens a checked regular document for reading, without blocking on a raced-in FIFO.
pub fn read(path: &Path) -> Result<File, String> {
    open(path, false)
}
/// Opens a walker-identified file and returns the size of the opened object.
/// Nonblocking open keeps a raced-in pipe from stalling a search worker.
pub(super) fn read_search_file(path: &Path) -> io::Result<(File, u64)> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(rustix::fs::OFlags::NONBLOCK.bits() as i32);
    }
    let file = options.open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(io::Error::other("only regular files can be searched"));
    }
    Ok((file, metadata.len()))
}
/// Opens a checked regular/new document for writing. The caller truncates only
/// after this descriptor check succeeds, never through OpenOptions::truncate.
pub fn write(path: &Path) -> Result<File, String> {
    open(path, true)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::{fs::symlink, net::UnixListener};

    #[test]
    fn checked_document_opens_reject_a_special_file_swapped_after_stat() {
        for kind in ["fifo", "socket", "device", "directory", "regular"] {
            for write in [false, true] {
                let directory = tempfile::tempdir().unwrap();
                let path = directory.path().join("document");
                std::fs::write(&path, "old").unwrap();
                let swapped = path.clone();
                BEFORE_OPEN.with(|hook| {
                    hook.replace(Some(Box::new(move || {
                        std::fs::remove_file(&swapped).unwrap();
                        match kind {
                            "fifo" => assert!(
                                std::process::Command::new("mkfifo")
                                    .arg(&swapped)
                                    .status()
                                    .unwrap()
                                    .success()
                            ),
                            "socket" => {
                                drop(UnixListener::bind(&swapped).unwrap());
                            }
                            "device" => symlink("/dev/zero", &swapped).unwrap(),
                            "directory" => std::fs::create_dir(&swapped).unwrap(),
                            "regular" => std::fs::write(&swapped, "different contents").unwrap(),
                            _ => unreachable!(),
                        }
                    })));
                });
                let start = std::time::Instant::now();
                let result = open(&path, write);
                assert!(start.elapsed() < std::time::Duration::from_secs(1));
                assert_eq!(
                    result.is_ok(),
                    kind == "regular" && !write,
                    "{kind}, write={write}"
                );
                if kind == "regular" {
                    assert_eq!(std::fs::read(&path).unwrap(), b"different contents");
                }
            }
        }
    }

    #[test]
    fn checked_regular_symlinks_clear_nonblock_and_cancel_history_decoding() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("document");
        let alias = directory.path().join("alias");
        std::fs::write(&path, "contents").unwrap();
        symlink(&path, &alias).unwrap();
        for file in [
            read(&path).unwrap(),
            read(&alias).unwrap(),
            write(&alias).unwrap(),
        ] {
            assert!(
                !rustix::fs::fcntl_getfl(&file)
                    .unwrap()
                    .contains(rustix::fs::OFlags::NONBLOCK)
            );
        }
        let cancel = vel_largefile::Cancellation::default();
        cancel.cancel();
        let result = crate::with_cancellation(cancel, || crate::load(&path, "UTF-8", |_| {}));
        assert!(result.is_err());
        assert!(crate::load(&path, "UTF-8", |_| {}).is_ok());
        assert!(std::fs::symlink_metadata(&alias).unwrap().is_symlink());
        let message = error(&path, "denied");
        assert_eq!(error(&path, &message), message);
        assert_eq!(error(&alias, &message), message);
    }
}
