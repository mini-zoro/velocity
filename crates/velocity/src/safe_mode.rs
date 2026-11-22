//! Isolate startup diagnostics and retain sessions implicated in a restore crash.
use serde::{Deserialize, Serialize};
#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::{
    fs::{self, DirBuilder, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Marker {
    pid: u32,
    started: u128,
}

#[derive(Debug)]
pub struct RestoreGuard {
    path: PathBuf,
    marker: Marker,
}

#[derive(Debug, Default)]
pub struct Startup {
    pub guard: Option<RestoreGuard>,
    pub skipped: Option<PathBuf>,
    pub warning: Option<String>,
    pub keep_original_closed: bool,
}

fn nanos(time: SystemTime) -> u128 {
    time.duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
}

pub fn profile(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .unwrap_or("Velocity".as_ref())
        .to_os_string();
    name.push(" (Safe Mode)");
    path.with_file_name(name)
}

pub fn fresh_run(profile: &Path) -> io::Result<PathBuf> {
    // https://www.sublimetext.com/docs/safe_mode.html
    // An empty run directory provides the clean settings/packages without
    // deleting any prior work. A safe-mode run does not have a session writer.
    private_directory(profile, true)?;
    if let Ok(entries) = fs::read_dir(profile) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let old = name
                .to_str()
                .and_then(|name| name.strip_prefix("Run-"))
                .and_then(|name| name.split_once('-'));
            if let Some((started, pid)) = old
                && started.parse::<u128>().is_ok()
                && pid.parse::<u32>().is_ok_and(dead)
                && entry.file_type().is_ok_and(|kind| kind.is_dir())
            {
                prune_empty_run(&entry.path(), 0);
            }
        }
    }
    let path = profile.join(format!(
        "Run-{}-{}",
        nanos(SystemTime::now()),
        std::process::id()
    ));
    private_directory(&path, false)?;
    Ok(path)
}

fn private_directory(path: &Path, recursive: bool) -> io::Result<()> {
    let mut builder = DirBuilder::new();
    builder.recursive(recursive);
    #[cfg(unix)]
    builder.mode(0o700);
    builder.create(path)
}

fn prune_empty_run(path: &Path, depth: usize) {
    if depth == 8
        || path
            .file_name()
            .is_some_and(|name| name.eq_ignore_ascii_case("Sessions"))
    {
        return;
    }
    if let Ok(entries) = fs::read_dir(path) {
        for entry in entries.flatten() {
            if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                prune_empty_run(&entry.path(), depth + 1);
            }
        }
    }
    // Never unlink files. A user may have saved work even in a diagnostic profile.
    let _ = fs::remove_dir(path);
}

fn read_marker(path: &Path) -> Option<Marker> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let file = options.open(path).ok()?;
    if !file.metadata().ok()?.is_file() {
        return None;
    }
    serde_json::from_reader(file.take(1024)).ok()
}

#[cfg(unix)]
fn dead(pid: u32) -> bool {
    let Ok(pid) = i32::try_from(pid) else {
        return false;
    };
    // SAFETY: signal zero only tests existence; never use process group IDs.
    pid > 1
        && unsafe { libc::kill(pid, 0) } == -1
        && io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
}

#[cfg(windows)]
fn dead(pid: u32) -> bool {
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use windows_sys::Win32::{
        Foundation::{ERROR_INVALID_PARAMETER, WAIT_OBJECT_0},
        Storage::FileSystem::SYNCHRONIZE,
        System::Threading::{OpenProcess, WaitForSingleObject},
    };
    if pid <= 4 {
        return false;
    }
    // SAFETY: request only permission to test for termination of this process ID.
    let handle = unsafe { OpenProcess(SYNCHRONIZE, 0, pid) };
    if handle.is_null() {
        return io::Error::last_os_error().raw_os_error() == Some(ERROR_INVALID_PARAMETER as i32);
    }
    // SAFETY: OpenProcess returned an owned handle which remains live across the wait.
    let handle = unsafe { OwnedHandle::from_raw_handle(handle) };
    // SAFETY: the wait only polls the owned process handle.
    unsafe { WaitForSingleObject(handle.as_raw_handle(), 0) == WAIT_OBJECT_0 }
}

fn crash_during(marker: Marker, reports: &[vel_crash::Report]) -> bool {
    // Only inspect reports returned by the crash reporter's public API.
    // The report name includes its process id; mtime excludes older reports
    // (the signal report's filename is reserved before the restore marker).
    reports.iter().any(|report| {
        report
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.split('-').nth(1))
            .and_then(|pid| pid.parse().ok())
            == Some(marker.pid)
            && report
                .path
                .metadata()
                .and_then(|m| m.modified())
                .is_ok_and(|time| nanos(time) >= marker.started)
    })
}

/// Called before loading a session, while holding the profile lock.
pub fn protect(profile: &Path, reports: &[vel_crash::Report]) -> Startup {
    protect_with(profile, reports, write_marker)
}

fn protect_with(
    profile: &Path,
    reports: &[vel_crash::Report],
    write: impl FnOnce(&Path, Marker) -> io::Result<()>,
) -> Startup {
    let directory = profile.join("Sessions");
    let session = directory.join("Session.sublime-session");
    let marker_path = directory.join("restore-in-progress");
    let exists = match session.symlink_metadata() {
        Ok(metadata) => metadata.is_file(),
        Err(error) if error.kind() == io::ErrorKind::NotFound => false,
        Err(error) => return protection_warning(error),
    };
    if !exists {
        return Startup::default();
    }
    if let Some(marker) = read_marker(&marker_path)
        && dead(marker.pid)
        && crash_during(marker, reports)
    {
        let skipped = directory.join(format!("Session.skipped-{}", nanos(SystemTime::now())));
        if let Err(error) = vel_fileio::rename_no_replace(&session, &skipped) {
            return Startup {
                warning: Some(format!(
                    "A previous restore crashed. The session was left intact at {}: {error}",
                    session.display()
                )),
                keep_original_closed: true,
                ..Startup::default()
            };
        }
        let _ = fs::remove_file(&marker_path);
        return Startup {
            skipped: Some(skipped),
            ..Startup::default()
        };
    }
    let marker = Marker {
        pid: std::process::id(),
        started: nanos(SystemTime::now()),
    };
    if let Err(error) = write(&marker_path, marker) {
        return protection_warning(error);
    }
    Startup {
        guard: Some(RestoreGuard {
            path: marker_path,
            marker,
        }),
        ..Startup::default()
    }
}

fn protection_warning(error: io::Error) -> Startup {
    Startup {
        warning: Some(format!(
            "Session restore is available, but crash-loop protection is off: {error}"
        )),
        ..Startup::default()
    }
}

fn write_marker(marker_path: &Path, marker: Marker) -> io::Result<()> {
    let tmp = marker_path.with_extension(format!("tmp-{}-{}", marker.pid, marker.started));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(&tmp)?;
    let result = (|| {
        file.write_all(&serde_json::to_vec(&marker).map_err(io::Error::other)?)?;
        file.sync_data()?;
        fs::rename(&tmp, marker_path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(tmp);
    }
    result
}

impl RestoreGuard {
    pub fn healthy_after_first_frame(self) {
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(3));
            if read_marker(&self.path) == Some(self.marker) {
                let _ = fs::remove_file(self.path);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dead_restore_with_matching_crash_quarantines_session_once() {
        let temp = tempfile::tempdir().unwrap();
        let dirs = temp.path().join("Sessions");
        fs::create_dir(&dirs).unwrap();
        let session = dirs.join("Session.sublime-session");
        fs::write(&session, "unsaved session bytes").unwrap();
        let pid = i32::MAX as u32;
        assert!(dead(pid));
        let marker = Marker { pid, started: 1 };
        let marker_path = dirs.join("restore-in-progress");
        fs::write(&marker_path, serde_json::to_vec(&marker).unwrap()).unwrap();
        let reporter =
            vel_crash::CrashReporter::new(vel_crash::Config::for_home(temp.path(), "fixture"))
                .unwrap();
        let report = reporter.directory().join(format!("2-{pid}-0-fatal.signal"));
        fs::write(&report, "simulated fatal signal").unwrap();
        let startup = protect(temp.path(), &reporter.pending_reports().unwrap());
        let backup = startup.skipped.unwrap();
        assert!(!session.exists());
        assert!(!marker_path.exists());
        assert_eq!(
            fs::read_to_string(&backup).unwrap(),
            "unsaved session bytes"
        );
        fs::write(&session, "new session").unwrap();
        // The stale marker must not quarantine the next normal session. A healthy marker
        // belongs to the new owner; a previous report is unrelated.
        assert!(
            protect(temp.path(), &reporter.pending_reports().unwrap())
                .skipped
                .is_none()
        );
        assert_eq!(fs::read_to_string(&session).unwrap(), "new session");
        assert_eq!(
            fs::read_to_string(&backup).unwrap(),
            "unsaved session bytes"
        );
    }

    #[test]
    #[cfg(unix)]
    fn read_only_sessions_and_enospc_leave_recovery_available() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("Sessions/Session.sublime-session");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut saved = vel_session::Session::default();
        saved
            .recent_files
            .push(temp.path().join("from-session.txt"));
        vel_session::save(&path, &saved).unwrap();
        let directory = path.parent().unwrap();
        fs::set_permissions(directory, fs::Permissions::from_mode(0o500)).unwrap();
        let read_only = protect(temp.path(), &[]);
        fs::set_permissions(directory, fs::Permissions::from_mode(0o700)).unwrap();
        for startup in [
            read_only,
            protect_with(temp.path(), &[], |_, _| {
                Err(io::Error::from_raw_os_error(libc::ENOSPC))
            }),
        ] {
            assert!(startup.guard.is_none());
            assert!(
                startup
                    .warning
                    .unwrap()
                    .contains("crash-loop protection is off")
            );
            assert!(!startup.keep_original_closed);
            let sessions = crate::session::Sessions::new(Some(temp.path()));
            assert_eq!(sessions.restored.recent_files, saved.recent_files);
        }
        assert!(path.exists());
        assert!(!directory.join("restore-in-progress").exists());
    }

    #[test]
    #[cfg(unix)]
    fn safe_mode_prunes_empty_dead_runs_without_touching_sessions_or_symlinks() {
        let temp = tempfile::tempdir().unwrap();
        let profile = profile(&temp.path().join("profile"));
        let empty = profile.join(format!("Run-1-{}", i32::MAX));
        fs::create_dir_all(empty.join("Packages/User")).unwrap();
        let preserved = profile.join(format!("Run-2-{}/Sessions", i32::MAX));
        fs::create_dir_all(&preserved).unwrap();
        let session = preserved.join("Session.sublime-session");
        fs::write(&session, "untouched").unwrap();
        let link = profile.join(format!("Run-3-{}", i32::MAX));
        std::os::unix::fs::symlink(&preserved, &link).unwrap();
        let live = profile.join(format!("Run-4-{}", std::process::id()));
        fs::create_dir(&live).unwrap();
        let run = fresh_run(&profile).unwrap();
        assert!(run.is_dir() && live.is_dir());
        assert!(!empty.exists());
        assert!(link.symlink_metadata().unwrap().is_symlink());
        assert_eq!(fs::read_to_string(session).unwrap(), "untouched");
    }
}
