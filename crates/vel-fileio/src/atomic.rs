//! The common staging and publication path for durable file replacements.
use std::{
    fs::{self, File},
    io,
    path::Path,
};

/// Strength of the flush before publication.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Durability {
    /// Request a hardware flush in addition to committing data to the OS.
    Full,
    /// Order writes without requesting a redundant cache flush on every checkpoint.
    Barrier,
    /// Submit this file's writes to the device. Follow with a barrier or full
    /// flush on that same device before modifying any data the file protects.
    Cached,
}

/// A complete save may have a weaker crash-durability guarantee if the directory can't be synced.
#[must_use]
#[derive(Debug)]
pub struct Published {
    /// Directory sync error, if any. The replacement itself has already succeeded.
    pub warning: Option<String>,
}

/// Same-directory staging file. Dropping it before publication leaves the destination alone.
#[derive(Debug)]
pub struct Stage(
    tempfile::NamedTempFile,
    #[cfg(unix)] Option<crate::save_journal::StageReference>,
);

impl Stage {
    #[cfg(unix)]
    pub(crate) fn create_at(path: &Path, permissions: Option<fs::Permissions>) -> io::Result<Self> {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .custom_flags(rustix::fs::OFlags::CLOEXEC.bits() as i32)
            .mode(permissions.map_or(0o600, |mode| mode.mode()))
            .open(path)?;
        let path = tempfile::TempPath::try_from_path(path)?;
        Ok(Self(tempfile::NamedTempFile::from_parts(file, path), None))
    }

    /// Creates a stage, defaulting to private permissions. An explicit mode is masked by umask.
    pub fn new(
        parent: &Path,
        prefix: &str,
        permissions: Option<fs::Permissions>,
    ) -> io::Result<Self> {
        let mut builder = tempfile::Builder::new();
        builder.prefix(prefix);
        if let Some(permissions) = permissions {
            builder.permissions(permissions);
        }
        let file = builder.tempfile_in(parent)?;
        Ok(Self(
            file,
            #[cfg(unix)]
            None,
        ))
    }

    /// The open staging descriptor.
    pub fn file(&mut self) -> &mut File {
        self.0.as_file_mut()
    }

    #[cfg(unix)]
    pub(crate) fn descriptor(&self) -> &File {
        self.0.as_file()
    }

    /// The private pathname, for filesystem operations on the staged file.
    pub fn path(&self) -> &Path {
        self.0.path()
    }

    /// Leaves the stage on disk for a published recovery journal.
    pub(crate) fn retain(&mut self) {
        self.0.disable_cleanup(true);
    }

    #[cfg(unix)]
    pub(crate) fn track(&mut self, reference: crate::save_journal::StageReference) {
        self.1 = Some(reference);
    }

    #[cfg(unix)]
    pub(crate) fn retain_for_recovery(&mut self) {
        self.retain();
        if let Some(reference) = &mut self.1 {
            reference.keep = true;
        }
    }

    /// Flushes the completed stage before making its name visible.
    pub fn sync(&self, durability: Durability) -> io::Result<()> {
        sync(self.0.as_file(), durability)
    }

    /// Publishes the already-synced stage by rename; never interprets a directory sync error
    /// as a failed replacement.
    pub fn publish(mut self, path: &Path, new: bool) -> io::Result<Published> {
        if !new {
            return self.replace_existing(path);
        }
        // https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/rename.2.html
        fault(Point::Publish)?;
        // A concurrent create is not permission to overwrite someone else's file.
        #[cfg(unix)]
        fault(Point::ExclusiveName)
            .and_then(|()| super::exclusive_rename(self.path(), path))
            .or_else(|e| {
                if unsupported(&e) {
                    fault(Point::ExclusiveName)
                        .and_then(|()| fs::hard_link(self.path(), path))
                        .or_else(|e| {
                            if unsupported(&e) {
                                self.publish_claimed(path)
                            } else {
                                Err(e)
                            }
                        })
                } else {
                    Err(e)
                }
            })?;
        #[cfg(not(unix))]
        self.0.persist_noclobber(path).map_err(|e| e.error)?;
        Ok(published(path))
    }

    #[cfg(unix)]
    fn publish_claimed(&mut self, path: &Path) -> io::Result<()> {
        // exFAT and some network filesystems have neither exclusive rename nor
        // hard links. The exclusive create never clobbers an existing name.
        let claim = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)?;
        let entry = claim.metadata()?;
        let ours = || {
            fs::symlink_metadata(path)
                .is_ok_and(|current| current.is_file() && super::same_entry(&entry, &current))
        };
        let result = if ours() {
            fault(Point::Replace).and_then(|()| fs::rename(self.path(), path))
        } else {
            Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "save name was replaced",
            ))
        };
        if result.is_ok() {
            self.retain();
        } else if ours() {
            fs::remove_file(path)?;
        }
        result
    }

    /// On failure the stage remains available for an inode-preserving save.
    pub(crate) fn replace_existing(&mut self, path: &Path) -> io::Result<Published> {
        fault(Point::Publish)?;
        fault(Point::Replace)?;
        fs::rename(self.path(), path)?;
        self.retain();
        Ok(published(path))
    }
}

fn published(path: &Path) -> Published {
    Published {
        warning: sync_parent(path).err().map(|e| {
            format!(
                "{} was saved, but its directory could not be synced: {e}",
                path.display()
            )
        }),
    }
}

/// Parent directory used for sibling temporaries.
pub fn parent(path: &Path) -> &Path {
    path.parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
}

/// Flush file data, using ordinary fsync when a filesystem doesn't implement the Darwin command.
#[allow(unsafe_code)]
pub fn sync(file: &File, durability: Durability) -> io::Result<()> {
    #[cfg(test)]
    FLUSHES.with(|trace| {
        if let Some(trace) = &mut *trace.borrow_mut() {
            trace.push(Some(durability));
        }
    });
    fault(Point::Sync)?;
    if durability == Durability::Cached {
        return fsync(file);
    }
    #[cfg(target_os = "macos")]
    {
        use std::os::fd::AsRawFd;
        // FULLFSYNC requests a device-cache flush. BARRIERFSYNC orders writes
        // on Apple storage but does not flush the device cache:
        // https://developer.apple.com/videos/play/wwdc2019/419/?time=766
        // https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/fcntl.2.html
        let command = match durability {
            Durability::Full => libc::F_FULLFSYNC,
            Durability::Barrier => libc::F_BARRIERFSYNC,
            Durability::Cached => unreachable!("ordinary fsync handled above"),
        };
        try_then_fsync(
            || loop {
                // SAFETY: this command ignores the variadic argument and the descriptor is live.
                let status = unsafe { libc::fcntl(file.as_raw_fd(), command, 0) };
                if status == 0 {
                    break Ok(());
                }
                let e = io::Error::last_os_error();
                if e.kind() != io::ErrorKind::Interrupted {
                    break Err(e);
                }
            },
            || fsync(file),
        )
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = durability;
        fsync(file)
    }
}

#[cfg(target_os = "macos")]
fn try_then_fsync(
    extended: impl FnOnce() -> io::Result<()>,
    ordinary: impl FnOnce() -> io::Result<()>,
) -> io::Result<()> {
    match extended() {
        Err(e) if unsupported(&e) => ordinary(),
        result => result,
    }
}

#[cfg(unix)]
pub(crate) fn unsupported(e: &io::Error) -> bool {
    use rustix::io::Errno;
    [Errno::NOTSUP, Errno::OPNOTSUPP, Errno::INVAL, Errno::NOSYS]
        .iter()
        .any(|code| Some(code.raw_os_error()) == e.raw_os_error())
}

fn fsync(file: &File) -> io::Result<()> {
    #[cfg(unix)]
    loop {
        match rustix::fs::fsync(file) {
            Err(rustix::io::Errno::INTR) => continue,
            result => break result.map_err(Into::into),
        }
    }
    #[cfg(not(unix))]
    file.sync_all()
}

pub(crate) fn sync_parent(path: &Path) -> io::Result<()> {
    #[cfg(test)]
    FLUSHES.with(|trace| {
        if let Some(trace) = &mut *trace.borrow_mut() {
            trace.push(None);
        }
    });
    #[cfg(test)]
    if DIRECTORY_DESTINATION.with(|target| target.borrow().as_ref().is_none_or(|p| p == path)) {
        fault(Point::Directory)?;
    }
    #[cfg(unix)]
    fsync(&File::open(parent(path))?)?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Point {
    Sync,
    Publish,
    #[cfg(unix)]
    ExclusiveName,
    Replace,
    #[cfg(test)]
    Directory,
}

fn fault(point: Point) -> io::Result<()> {
    #[cfg(all(test, unix))]
    if point == Point::ExclusiveName && EXFAT_PROBE.get() {
        return Err(rustix::io::Errno::NOTSUP.into());
    }
    #[cfg(test)]
    if let Some(code) = FAULT.with(|slot| slot.get().filter(|(p, _)| *p == point).map(|(_, e)| e)) {
        return Err(io::Error::from_raw_os_error(code));
    }
    let _ = point;
    Ok(())
}

#[cfg(test)]
thread_local! {
    pub(crate) static DIRECTORY_DESTINATION: std::cell::RefCell<Option<std::path::PathBuf>> = const { std::cell::RefCell::new(None) };
    pub(crate) static FLUSHES: std::cell::RefCell<Option<Vec<Option<Durability>>>> = const { std::cell::RefCell::new(None) };
}

#[cfg(all(test, unix))]
#[test]
fn exclusive_create_is_the_last_resort_and_failed_rename_releases_only_its_claim() {
    use std::io::Write;
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("new");
    let stage = || {
        let mut s = Stage::new(root.path(), ".velocity-save-", None).unwrap();
        s.file().write_all(b"complete").unwrap();
        s.sync(Durability::Full).unwrap();
        s
    };
    EXFAT_PROBE.set(true);
    FAULT.set(Some((Point::Replace, rustix::io::Errno::IO.raw_os_error())));
    assert!(stage().publish(&path, true).is_err());
    assert!(!path.exists());
    FAULT.set(None);
    let _ = stage().publish(&path, true).unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"complete");
    assert_eq!(
        stage().publish(&path, true).unwrap_err().kind(),
        io::ErrorKind::AlreadyExists
    );
    assert_eq!(fs::read(&path).unwrap(), b"complete");
    EXFAT_PROBE.set(false);
}

#[cfg(test)]
thread_local! {
    pub(crate) static FAULT: std::cell::Cell<Option<(Point, i32)>> = const { std::cell::Cell::new(None) };
    #[cfg(unix)]
    pub(crate) static EXFAT_PROBE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[cfg(all(test, target_os = "macos"))]
#[test]
fn unsupported_full_flush_retries_but_io_failure_does_not() {
    assert!(
        try_then_fsync(
            || Err(io::Error::from_raw_os_error(libc::ENOTSUP)),
            || Ok(())
        )
        .is_ok()
    );
    assert!(
        try_then_fsync(
            || Err(io::Error::from_raw_os_error(libc::EIO)),
            || panic!("must not hide EIO")
        )
        .is_err()
    );
}
