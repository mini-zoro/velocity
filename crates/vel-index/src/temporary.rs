//! Ownership of expendable index caches, including detached coordinators at process exit.
use std::{
    fs, io,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};
use tempfile::TempDir;

#[derive(Default)]
struct Directories {
    next: u64,
    exiting: bool,
    active: Vec<TempDir>,
}

fn directories() -> &'static Mutex<Directories> {
    static DIRS: OnceLock<Mutex<Directories>> = OnceLock::new();
    DIRS.get_or_init(Mutex::default)
}

extern "C" fn cleanup_on_exit() {
    let mut dirs = directories().lock().unwrap_or_else(|e| e.into_inner());
    dirs.exiting = true;
    dirs.active.clear();
}

#[derive(Debug)]
pub(crate) struct TemporaryIndex(PathBuf);

impl TemporaryIndex {
    pub(crate) fn new(root: &Path) -> io::Result<Self> {
        static REGISTERED: OnceLock<bool> = OnceLock::new();
        // SAFETY: callback has the C ABI and static storage; it does not unwind.
        if !*REGISTERED.get_or_init(|| unsafe { libc::atexit(cleanup_on_exit) } == 0) {
            return Err(io::Error::other("could not register index cleanup"));
        }
        let root = root.canonicalize()?;
        // Abandoned directories from crashed processes are swept once per process, off the
        // caller's thread: only processes that actually create a temporary index pay for the
        // temp-root scan, and never on the startup path.
        static SWEPT: std::sync::Once = std::sync::Once::new();
        SWEPT.call_once(|| {
            let sweep_root = root.clone();
            let _ = std::thread::Builder::new()
                .name("velocity-index-sweep".into())
                .spawn(move || sweep_stale(&sweep_root));
        });
        let mut dirs = directories().lock().unwrap_or_else(|e| e.into_inner());
        if dirs.exiting {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "process exiting",
            ));
        }
        // Never reuse a name, including a stale directory from a recycled pid.
        loop {
            let name = format!(
                "velocity-index-fallback-{}-{}",
                std::process::id(),
                dirs.next
            );
            dirs.next += 1;
            let mut builder = tempfile::Builder::new();
            builder.prefix(&name).rand_bytes(0);
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                builder.permissions(fs::Permissions::from_mode(0o700));
            }
            match builder.tempdir_in(&root) {
                Ok(dir) => {
                    let path = dir.path().to_owned();
                    dirs.active.push(dir);
                    return Ok(Self(path));
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        }
    }

    pub(crate) fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TemporaryIndex {
    fn drop(&mut self) {
        let mut dirs = directories().lock().unwrap_or_else(|e| e.into_inner());
        if let Some(index) = dirs.active.iter().position(|dir| dir.path() == self.0) {
            dirs.active.swap_remove(index);
        }
    }
}

/// Removes abandoned temporary index directories from this process's temp root.
/// A directory is eligible only for our effective uid, a known name and an absent pid.
pub fn sweep_stale(root: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let Ok(entries) = fs::read_dir(root) else {
            return;
        };
        // SAFETY: geteuid takes no arguments and has no side effects.
        let uid = unsafe { libc::geteuid() };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(pid) = name.to_str().and_then(index_pid) else {
                continue;
            };
            let path = entry.path();
            if let Ok(meta) = fs::symlink_metadata(&path)
                && meta.is_dir()
                && meta.uid() == uid
                && pid_is_absent(pid)
            {
                // std's Unix recursive removal does not follow symlinks in the tree.
                let _ = fs::remove_dir_all(path);
            }
        }
    }
    #[cfg(not(unix))]
    let _ = root;
}

#[cfg(unix)]
fn index_pid(name: &str) -> Option<libc::pid_t> {
    fn canonical_decimal(s: &str) -> bool {
        !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) && (s == "0" || !s.starts_with('0'))
    }
    let pid = if let Some(fallback) = name.strip_prefix("velocity-index-fallback-") {
        let (pid, count) = fallback.split_once('-')?;
        if !canonical_decimal(count) || count.parse::<u64>().is_err() {
            return None;
        }
        pid
    } else {
        name.strip_prefix("velocity-index-")?
    };
    if !canonical_decimal(pid) {
        return None;
    }
    pid.parse().ok().filter(|pid| *pid > 0)
}

#[cfg(unix)]
fn pid_is_absent(pid: libc::pid_t) -> bool {
    // SAFETY: index_pid supplies a positive pid. Signal 0 only probes existence.
    let result = unsafe { libc::kill(pid, 0) };
    result == -1 && io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
}
