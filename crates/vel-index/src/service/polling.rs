use crate::store::Stamp;
use std::{
    collections::BTreeMap,
    ops::Bound::{Excluded, Unbounded},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

const INTERVAL: Duration = Duration::from_millis(100);
// Do not catch up missed ticks. Account for directory, inherited and file stats
// out of the same budget even when a walk or a network request stalls.
const BUDGET: usize = 240;
const DIRECTORY_BUDGET: usize = 80;
pub(super) const REPROBE: Duration = Duration::from_secs(60);

#[cfg(target_os = "macos")]
pub(super) fn local_volume(path: &Path) -> std::io::Result<bool> {
    use std::{ffi::CString, mem::MaybeUninit, os::unix::ffi::OsStrExt};
    // FSEvents doesn't monitor a remote SMB/NFS root. `statfs` describes the
    // filesystem containing the root, not the cache or the profile.
    // https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/statfs.2.html
    let path = CString::new(path.as_os_str().as_bytes())?;
    let mut info = MaybeUninit::<libc::statfs>::uninit();
    // SAFETY: path is NUL-terminated and a successful call initializes info.
    if unsafe { libc::statfs(path.as_ptr(), info.as_mut_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: statfs returned success above.
    let info = unsafe { info.assume_init() };
    Ok(flags_are_local(info.f_flags))
}

#[cfg(target_os = "macos")]
fn flags_are_local(flags: u32) -> bool {
    flags & libc::MNT_LOCAL as u32 != 0
}

#[cfg(not(target_os = "macos"))]
pub(super) fn local_volume(_: &Path) -> std::io::Result<bool> {
    Ok(true)
}

pub(super) type Stamps = BTreeMap<PathBuf, Option<Stamp>>;

pub(super) struct Polling {
    pub(super) next: Instant,
    inherited: usize,
    directory: Option<PathBuf>,
    file: Option<PathBuf>,
}

impl Polling {
    pub(super) fn new() -> Self {
        Self {
            next: Instant::now() + INTERVAL,
            inherited: 0,
            directory: None,
            file: None,
        }
    }

    /// Directory changes trigger subtree walks. Unchanged directories need no
    /// walk, but an in-place save doesn't update their mtime. Sweep file stamps
    /// separately, incrementally, so those changes are still detected.
    pub(super) fn tick(
        &mut self,
        now: Instant,
        inherited: &[(PathBuf, Option<Stamp>)],
        directories: &Stamps,
        files: &Stamps,
        mut directory_changed: impl FnMut(&Path, Option<Stamp>),
        mut file_changed: impl FnMut(&Path, Option<Stamp>),
    ) -> bool {
        if now < self.next {
            return false;
        }
        self.next = now + INTERVAL;
        let mut rescan = false;
        let inherited_budget = inherited.len().min(DIRECTORY_BUDGET / 4);
        for _ in 0..inherited_budget {
            self.inherited %= inherited.len();
            let (path, stamp) = &inherited[self.inherited];
            self.inherited += 1;
            rescan |= Stamp::read(path).ok() != *stamp;
        }
        let dirs = visit(
            directories,
            &mut self.directory,
            DIRECTORY_BUDGET - inherited_budget,
            |path, stamp| {
                let current = Stamp::read(path).ok();
                if current != stamp {
                    directory_changed(path, current);
                }
            },
        );
        visit(
            files,
            &mut self.file,
            BUDGET - inherited_budget - dirs,
            |path, stamp| {
                file_changed(path, stamp);
            },
        );
        rescan
    }
}

fn visit(
    entries: &Stamps,
    cursor: &mut Option<PathBuf>,
    budget: usize,
    mut callback: impl FnMut(&Path, Option<Stamp>),
) -> usize {
    let mut count = 0;
    let mut last = None;
    let previous = cursor.as_deref();
    let end = previous.map_or(Unbounded, Excluded);
    let limit = budget.min(entries.len());
    for (path, stamp) in entries
        .range::<Path, _>((end, Unbounded))
        .chain(
            entries
                .iter()
                .take_while(|(path, _)| previous.is_some_and(|last| path.as_path() <= last)),
        )
        .take(limit)
    {
        callback(path, *stamp);
        last = Some(path);
        count += 1;
    }
    if let Some(path) = last {
        if let Some(cursor) = cursor.as_mut() {
            cursor.clone_from(path);
        } else {
            *cursor = Some(path.clone());
        }
    }
    count
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[test]
    fn network_mount_flags_require_polling() {
        assert!(!flags_are_local(libc::MNT_RDONLY as u32));
        assert!(flags_are_local((libc::MNT_LOCAL | libc::MNT_RDONLY) as u32));
        let dir = tempfile::tempdir().unwrap();
        assert!(local_volume(dir.path()).unwrap());
    }
}
