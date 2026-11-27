use crate::{
    Client, Error, HELPER, Offer, Platform, Reason, Result, release::ARCHIVE_LIMIT, verify,
};
use semver::Version;
use std::{
    ffi::OsString,
    fs::{self, File},
    io::{self, Read},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

pub const STARTUP: &str = "VELOCITY_UPDATE_STARTUP";
pub const RECOVERY: &str = "VELOCITY_UPDATE_RECOVERY";
const WAIT: Duration = Duration::from_secs(120);
const ZIP: &str = "release.zip";
const SUMS: &str = "SHA256SUMS";
const SIG: &str = "SHA256SUMS.sig";

#[derive(Debug)]
pub struct Install {
    target: PathBuf,
    platform: Platform,
    current: Version,
}

#[derive(Debug)]
pub struct Stage {
    work: tempfile::TempDir,
    install: Install,
    data: PathBuf,
    offer: Offer,
    helper_digest: [u8; 32],
}

pub(crate) fn updates_dir(data: &Path) -> io::Result<PathBuf> {
    fs::create_dir_all(data)?;
    let updates = data.canonicalize()?.join("Updates");
    directory(&updates)?;
    Ok(updates)
}

fn directory(path: &Path) -> io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    match builder.create(path) {
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => plain(path, true),
        result => result,
    }
}

fn private_tempdir(parent: &Path, prefix: &str) -> io::Result<tempfile::TempDir> {
    let mut builder = tempfile::Builder::new();
    builder.prefix(prefix);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        builder.permissions(fs::Permissions::from_mode(0o700));
    }
    builder.tempdir_in(parent)
}

fn plain(path: &Path, directory: bool) -> io::Result<()> {
    let meta = path.symlink_metadata()?;
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if meta.file_attributes() & 0x400 != 0 {
            // FILE_ATTRIBUTE_REPARSE_POINT
            return Err(io::Error::other("unexpected reparse point"));
        }
    }
    if (directory && meta.is_dir()) || (!directory && meta.is_file()) {
        Ok(())
    } else {
        Err(io::Error::other("unexpected file type in update path"))
    }
}

fn unsupported() -> Error {
    Error::new(
        Reason::ManualInstall,
        "not a recognized portable or macOS bundle installation",
    )
}

impl Install {
    pub fn detect(exe: &Path, platform: Platform, current: &str) -> Result<Self> {
        let exe = exe.canonicalize()?;
        let base = exe.parent().ok_or_else(unsupported)?;
        let target = match platform {
            Platform::Mac => base
                .parent()
                .and_then(Path::parent)
                .ok_or_else(unsupported)?,
            Platform::Windows => base,
        };
        if executable(target, platform) != exe
            || (platform == Platform::Mac
                && target
                    .extension()
                    .is_none_or(|ext| !ext.eq_ignore_ascii_case("app")))
        {
            return Err(unsupported());
        }
        let current = Version::parse(current).map_err(|_| unsupported())?;
        validate_target(target, platform, &current).map_err(|_| unsupported())?;
        let install = Self {
            target: target.into(),
            platform,
            current,
        };
        install.writable()?;
        Ok(install)
    }

    fn writable(&self) -> Result<()> {
        // Probe actual ACL/mount writability (not just POSIX permission bits).
        let parent = self.target.parent().ok_or_else(unsupported)?;
        tempfile::Builder::new()
            .prefix(".velocity-probe-")
            .tempdir_in(parent)
            .and_then(tempfile::TempDir::close)
            .map_err(|e| Error::new(Reason::NotWritable, e))?;
        absent(&old_path(&self.target)?)?;
        Ok(())
    }

    pub fn stage(self, client: &Client, data: &Path, offer: Offer) -> Result<Stage> {
        if offer.platform != self.platform || !offer.version.cmp_precedence(&self.current).is_gt() {
            return Err(unsupported());
        }
        self.writable()?;
        let updates = updates_dir(data)?;
        if updates.starts_with(&self.target) {
            return Err(unsupported());
        }
        prune_completed(&updates);
        let version_dir = updates.join(offer.version.to_string());
        directory(&version_dir)?;
        let work = private_tempdir(&version_dir, "stage-")?;
        fs::write(work.path().join(SUMS), &offer.proof.0)?;
        fs::write(work.path().join(SIG), &offer.proof.1)?;
        let archive = work.path().join(ZIP);
        client.download(
            &offer.asset_url,
            &mut File::create(&archive)?,
            ARCHIVE_LIMIT,
        )?;
        let unpack = work.path().join("verified");
        let candidate = extract(
            &archive,
            &unpack,
            self.platform,
            &offer.version,
            &offer.digest,
        )?;
        let helper_digest = verify::digest(File::open(executable(&candidate, self.platform))?)?;
        Ok(Stage {
            work,
            install: self,
            data: data.canonicalize()?,
            offer,
            helper_digest,
        })
    }
}

impl Stage {
    pub fn launch(self) -> Result<()> {
        let executable = executable(
            &self
                .work
                .path()
                .join("verified")
                .join(root(self.install.platform)),
            self.install.platform,
        );
        // An opted-in update may have been staged long before Quit.
        plain(&executable, false)?;
        verify::check_digest(File::open(&executable)?, &self.helper_digest)?;
        let mut command = Command::new(executable);
        detached_io(&mut command, self.work.path());
        command
            .arg(HELPER)
            .arg(self.work.path())
            .arg(&self.install.target)
            .arg(std::process::id().to_string())
            .arg(&self.data)
            .arg(self.install.current.to_string())
            .arg(self.offer.version.to_string());
        command.spawn()?;
        // The helper, not this process, now owns cleanup.
        let _ = self.work.keep();
        Ok(())
    }
}

fn root(platform: Platform) -> &'static str {
    match platform {
        Platform::Mac => "Velocity.app",
        Platform::Windows => "Velocity",
    }
}

fn executable(path: &Path, platform: Platform) -> PathBuf {
    path.join(match platform {
        Platform::Mac => "Contents/MacOS/velocity",
        Platform::Windows => "velocity.exe",
    })
}

fn validate_target(path: &Path, platform: Platform, version: &Version) -> Result<()> {
    match platform {
        Platform::Mac => validate_bundle(path, version),
        Platform::Windows => validate_portable(path, version),
    }
}

fn validate_portable(path: &Path, version: &Version) -> Result<()> {
    plain(path, true)?;
    // A directory swap must not move projects or uninstallers in with the old ZIP.
    for entry in fs::read_dir(path)? {
        if !matches!(
            entry?.file_name().to_str(),
            Some("velocity.exe" | "vel.exe" | "Resources" | "README-Windows.txt")
        ) {
            return Err(unsupported());
        }
    }
    plain(&path.join("Resources"), true)?;
    for name in [
        "velocity.exe",
        "vel.exe",
        "README-Windows.txt",
        "Resources/build-info.json",
    ] {
        plain(&path.join(name), false)?;
    }
    #[derive(serde::Deserialize)]
    struct BuildInfo {
        version: Version,
        target: String,
    }
    let info = bounded_read(&path.join("Resources/build-info.json"), 65536)?;
    let info: BuildInfo =
        serde_json::from_slice(info.strip_prefix(b"\xef\xbb\xbf").unwrap_or(&info))
            .map_err(|e| Error::new(Reason::Staging, e))?;
    if info.version != *version || info.target != "x86_64-pc-windows-msvc" {
        return Err(unsupported());
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn validate_bundle(path: &Path, version: &Version) -> Result<()> {
    for dir in [
        path.to_owned(),
        path.join("Contents"),
        path.join("Contents/MacOS"),
    ] {
        plain(&dir, true)?;
    }
    let info = path.join("Contents/Info.plist");
    plain(&info, false)?;
    plain(&executable(path, Platform::Mac), false)?;
    #[derive(serde::Deserialize)]
    struct Info {
        #[serde(rename = "CFBundleIdentifier")]
        id: String,
        #[serde(rename = "CFBundleShortVersionString")]
        version: String,
        #[serde(rename = "CFBundleExecutable")]
        executable: String,
    }
    let info: Info = plist::from_file(info).map_err(|e| Error::new(Reason::Staging, e))?;
    let expected_numeric = format!("{}.{}.{}", version.major, version.minor, version.patch);
    plain(&path.join("Contents/Resources"), true)?;
    let build: serde_json::Value = serde_json::from_slice(&bounded_read(
        &path.join("Contents/Resources/build-info.json"),
        65536,
    )?)
    .map_err(|e| Error::new(Reason::Staging, e))?;
    if info.id != "dev.velocity.Velocity"
        || info.version != expected_numeric
        || info.executable != "velocity"
        || build["version"].as_str() != Some(&version.to_string())
    {
        return Err(Error::new(
            Reason::Staging,
            "wrong macOS bundle identifier, version, or executable",
        ));
    }
    Ok(())
}

fn bounded_read(path: &Path, limit: u64) -> io::Result<Vec<u8>> {
    plain(path, false)?;
    let mut bytes = Vec::new();
    let count = File::open(path)?.take(limit + 1).read_to_end(&mut bytes)? as u64;
    if count > limit {
        return Err(io::Error::other("oversized update metadata"));
    }
    Ok(bytes)
}

#[cfg(not(target_os = "macos"))]
fn validate_bundle(_path: &Path, _version: &Version) -> Result<()> {
    Err(unsupported())
}

fn zip_path(name: &str, platform: Platform) -> Result<()> {
    let bad = || Error::new(Reason::Staging, "unsafe release archive path");
    let name = name.strip_suffix('/').unwrap_or(name);
    let mut components = name.split('/');
    let first = components.next().ok_or_else(bad)?;
    if first != root(platform) && !(platform == Platform::Mac && first == "__MACOSX") {
        return Err(bad());
    }
    for part in components {
        if part.is_empty()
            || part == "."
            || part == ".."
            || part.contains(['\\', ':'])
            || part.chars().any(char::is_control)
        {
            return Err(bad());
        }
        if platform == Platform::Windows {
            let base = part
                .split('.')
                .next()
                .unwrap_or_default()
                .trim_end_matches(' ')
                .to_ascii_uppercase();
            if part.ends_with([' ', '.'])
                || part.contains(['<', '>', '"', '|', '?', '*'])
                || matches!(
                    base.as_str(),
                    "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
                )
                || (base.starts_with("COM") || base.starts_with("LPT"))
                    && base[3..].chars().count() == 1
                    && base[3..]
                        .chars()
                        .any(|c| c.is_ascii_digit() || "¹²³".contains(c))
            {
                return Err(bad());
            }
        }
    }
    Ok(())
}

fn extract(
    archive: &Path,
    unpack: &Path,
    platform: Platform,
    version: &Version,
    digest: &[u8; 32],
) -> Result<PathBuf> {
    use std::io::Seek;
    let mut file = File::open(archive)?;
    verify::check_digest(&mut file, digest)?;
    file.rewind()?;
    let zip = |e| Error::new(Reason::Staging, e);
    let mut entries = zip::ZipArchive::new(file).map_err(zip)?;
    if entries.len() > 100_000 {
        return Err(Error::new(Reason::Staging, "too many ZIP entries"));
    }
    let mut remaining = 2 * 1024 * 1024 * 1024u64;
    fs::create_dir(unpack)?;
    for index in 0..entries.len() {
        let mut item = entries.by_index(index).map_err(zip)?;
        zip_path(item.name(), platform)?;
        if item
            .unix_mode()
            .is_some_and(|mode| !matches!(mode & 0o170000, 0 | 0o100000 | 0o040000))
        {
            return Err(Error::new(Reason::Staging, "non-file ZIP entry"));
        }
        let expanded = item.size();
        remaining = remaining
            .checked_sub(expanded)
            .ok_or_else(|| Error::new(Reason::Staging, "expanded ZIP is too large"))?;
        if platform == Platform::Windows {
            let output = unpack.join(item.name());
            if item.is_dir() {
                fs::create_dir_all(output)?;
            } else {
                fs::create_dir_all(output.parent().expect("ZIP entry under root"))?;
                let mut destination = File::create_new(output)?;
                if io::copy(&mut (&mut item).take(expanded + 1), &mut destination)? != expanded {
                    return Err(Error::new(Reason::Staging, "invalid ZIP entry size"));
                }
            }
        }
    }
    let candidate = unpack.join(root(platform));
    if platform == Platform::Mac {
        mac_tool("/usr/bin/ditto", |command| {
            command.args(["-x", "-k"]).arg(archive).arg(unpack)
        })?;
        validate_bundle(&candidate, version)?;
        mac_tool("/usr/bin/codesign", |command| {
            command
                .args(["--verify", "--deep", "--strict"])
                .arg(&candidate)
        })?;
    } else {
        validate_portable(&candidate, version)?;
    }
    Ok(candidate)
}

fn mac_tool(program: &str, customize: impl FnOnce(&mut Command) -> &mut Command) -> Result<()> {
    let mut command = Command::new(program);
    if !customize(&mut command)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?
        .success()
    {
        return Err(Error::new(
            Reason::Staging,
            format_args!("{program} failed"),
        ));
    }
    Ok(())
}

fn old_path(target: &Path) -> Result<PathBuf> {
    let mut basename = target.file_name().ok_or_else(unsupported)?.to_owned();
    basename.push(".old");
    Ok(target.with_file_name(basename))
}

fn absent(path: &Path) -> io::Result<()> {
    match path.symlink_metadata() {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
        Ok(_) => Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("backup already exists: {}", path.display()),
        )),
    }
}

fn detached_io(command: &mut Command, dir: &Path) {
    command
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .env_remove(STARTUP)
        .env_remove(RECOVERY);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW (does not hide GUI apps)
    }
}

fn launch(
    target: &Path,
    platform: Platform,
    data: &Path,
    marker: Option<&Path>,
) -> io::Result<Child> {
    let mut command = Command::new(executable(target, platform));
    detached_io(&mut command, target.parent().unwrap_or(target));
    command.arg("--data-dir").arg(data).arg("--restore-session");
    if let Some(marker) = marker {
        command.env(STARTUP, marker);
    } else {
        command.env(RECOVERY, "1");
    }
    command.spawn()
}

fn startup(child: &mut Child, marker: &Path, timeout: Duration) -> bool {
    let start = Instant::now();
    let expected = child.id().to_string();
    loop {
        if fs::read_to_string(marker).is_ok_and(|value| value == expected) {
            return true;
        }
        match child.try_wait() {
            Ok(Some(_)) => return false,
            Ok(None) => (),
            Err(_) => break,
        }
        if start.elapsed() >= timeout {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let _ = child.kill();
    let _ = child.wait();
    false
}

// Same-volume names only. macOS can exchange both names in one operation;
// Windows must restore the first rename if the second one fails.
fn swap(target: &Path, candidate: &Path, backup: &Path, platform: Platform) -> io::Result<()> {
    absent(backup)?;
    if platform == Platform::Mac {
        fs::rename(candidate, backup)?;
        if let Err(error) = exchange(target, backup) {
            fs::rename(backup, candidate)?;
            return Err(error);
        }
    } else {
        fs::rename(target, backup)?;
        if let Err(error) = fs::rename(candidate, target) {
            fs::rename(backup, target)?;
            return Err(error);
        }
    }
    Ok(())
}

fn rollback(target: &Path, backup: &Path, displaced: &Path, platform: Platform) -> io::Result<()> {
    if platform == Platform::Mac {
        exchange(target, backup)?;
        fs::rename(backup, displaced)?;
    } else {
        fs::rename(target, displaced)?;
        if let Err(error) = fs::rename(backup, target) {
            fs::rename(displaced, target)?;
            return Err(error);
        }
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn exchange(a: &Path, b: &Path) -> io::Result<()> {
    use std::{ffi::CString, os::unix::ffi::OsStrExt};
    let a = CString::new(a.as_os_str().as_bytes())?;
    let b = CString::new(b.as_os_str().as_bytes())?;
    // SAFETY: both strings are live NUL-terminated paths, with no aliased output.
    if unsafe { libc::renamex_np(a.as_ptr(), b.as_ptr(), libc::RENAME_SWAP) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(not(target_os = "macos"))]
fn exchange(_a: &Path, _b: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "bundle exchange requires macOS",
    ))
}

/// Entry point before the editor attempts to acquire its single-instance lock.
pub fn apply_update(args: impl IntoIterator<Item = OsString>) -> Result<()> {
    let args: [OsString; 6] = args
        .into_iter()
        .collect::<Vec<_>>()
        .try_into()
        .map_err(|_| unsupported())?;
    let [work, target, pid, data, current, next] = args;
    let (work, target, data) = (
        PathBuf::from(work),
        PathBuf::from(target),
        PathBuf::from(data),
    );
    let pid = pid
        .to_str()
        .and_then(|p| p.parse::<u32>().ok())
        .filter(|&p| p > 4 && p != std::process::id())
        .ok_or_else(unsupported)?;
    let parse = |v: &OsString| {
        v.to_str()
            .and_then(|v| Version::parse(v).ok())
            .ok_or_else(unsupported)
    };
    let (current, next) = (parse(&current)?, parse(&next)?);
    if !next.cmp_precedence(&current).is_gt()
        || !next.pre.is_empty()
        || next.to_string() != env!("CARGO_PKG_VERSION")
    {
        return Err(unsupported());
    }
    let updates = updates_dir(&data)?;
    let platform = Platform::current();
    if work.parent() != Some(updates.join(next.to_string()).as_path())
        || !work
            .file_name()
            .is_some_and(|n| n.to_string_lossy().starts_with("stage-"))
        || fs::canonicalize(&work)? != work
        || fs::canonicalize(&target)? != target
        || work.starts_with(&target)
        || fs::canonicalize(std::env::current_exe()?)?
            != executable(&work.join("verified").join(root(platform)), platform)
    {
        return Err(unsupported());
    }
    wait_for_exit(pid, WAIT)?;
    let result = (|| {
        let digest = verify::signed_digest(
            &bounded_read(&work.join(SUMS), 65536)?,
            &bounded_read(&work.join(SIG), 1024)?,
            &verify::trusted_keys()?,
            &platform.asset(&next),
        )?;
        replace(&work, &target, &data, platform, &current, &next, &digest)
    })();
    if result.is_err() && validate_target(&target, platform, &current).is_ok() {
        let _ = launch(&target, platform, &data, None);
    }
    // A running helper cannot remove its own image on Windows. Reap marked work on a later stage.
    let _ = fs::remove_dir_all(&work);
    if work.exists() {
        let _ = fs::write(work.join("finished"), b"");
    }
    result
}

fn replace(
    work: &Path,
    target: &Path,
    data: &Path,
    platform: Platform,
    current: &Version,
    next: &Version,
    digest: &[u8; 32],
) -> Result<()> {
    validate_target(target, platform, current)?;
    let parent = target.parent().ok_or_else(unsupported)?;
    let backup = old_path(target)?;
    absent(&backup)?;
    let reservation = private_tempdir(parent, ".velocity-update-")
        .map_err(|e| Error::new(Reason::NotWritable, e))?;
    // Re-read the verified ZIP after quit; never swap arbitrary files from a previous stage.
    let candidate = extract(
        &work.join(ZIP),
        &reservation.path().join("new"),
        platform,
        next,
        digest,
    )?;
    let marker = work.join("started");
    absent(&marker)?;
    trial_replace(target, &candidate, &backup, platform, || {
        launch(target, platform, data, Some(&marker))
            .is_ok_and(|mut child| startup(&mut child, &marker, WAIT))
    })
}

fn trial_replace(
    target: &Path,
    candidate: &Path,
    backup: &Path,
    platform: Platform,
    confirm: impl FnOnce() -> bool,
) -> Result<()> {
    swap(target, candidate, backup, platform)?;
    if confirm() {
        // This is the only success path that removes the previous app.
        fs::remove_dir_all(backup)?;
        return Ok(());
    }
    rollback(target, backup, candidate, platform)?;
    Err(Error::new(
        Reason::Staging,
        "replacement did not confirm startup; restored previous version",
    ))
}

fn prune_completed(updates: &Path) {
    let Ok(versions) = fs::read_dir(updates) else {
        return;
    };
    for version in versions.flatten().filter(|e| {
        e.file_name()
            .to_str()
            .is_some_and(|n| Version::parse(n).is_ok())
    }) {
        if plain(&version.path(), true).is_err() {
            continue;
        }
        let Ok(stages) = fs::read_dir(version.path()) else {
            continue;
        };
        for stage in stages
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with("stage-"))
        {
            let path = stage.path();
            let old = stage
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|time| time.elapsed().ok())
                .is_some_and(|age| age >= Duration::from_secs(7 * 24 * 60 * 60));
            if plain(&path, true).is_ok() && (old || plain(&path.join("finished"), false).is_ok()) {
                let _ = fs::remove_dir_all(stage.path());
            }
        }
        let _ = fs::remove_dir(version.path());
    }
}

#[cfg(target_os = "macos")]
fn wait_for_exit(pid: u32, timeout: Duration) -> io::Result<()> {
    let start = Instant::now();
    let pid = i32::try_from(pid).map_err(io::Error::other)?;
    loop {
        // SAFETY: zero signal only queries the positive PID; it never delivers a signal.
        if unsafe { libc::kill(pid, 0) } < 0 {
            let error = io::Error::last_os_error();
            return if error.raw_os_error() == Some(libc::ESRCH) {
                Ok(())
            } else {
                Err(error)
            };
        }
        if start.elapsed() >= timeout {
            return Err(wait_timeout());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[cfg(windows)]
fn wait_for_exit(pid: u32, timeout: Duration) -> io::Result<()> {
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use windows_sys::Win32::{
        Foundation::{ERROR_INVALID_PARAMETER, WAIT_OBJECT_0, WAIT_TIMEOUT},
        Storage::FileSystem::SYNCHRONIZE,
        System::Threading::{OpenProcess, WaitForSingleObject},
    };
    // SAFETY: only the synchronization right for the given PID is requested.
    let handle = unsafe { OpenProcess(SYNCHRONIZE, 0, pid) };
    if handle.is_null() {
        let error = io::Error::last_os_error();
        return if error.raw_os_error() == Some(ERROR_INVALID_PARAMETER as i32) {
            Ok(())
        } else {
            Err(error)
        };
    }
    // SAFETY: OpenProcess returned a new process handle to close after the wait.
    let handle = unsafe { OwnedHandle::from_raw_handle(handle) };
    // SAFETY: the owned process handle outlives the synchronous wait.
    match unsafe { WaitForSingleObject(handle.as_raw_handle(), timeout.as_millis() as u32) } {
        WAIT_OBJECT_0 => Ok(()),
        WAIT_TIMEOUT => Err(wait_timeout()),
        _ => Err(io::Error::last_os_error()),
    }
}

#[cfg(any(target_os = "macos", windows))]
fn wait_timeout() -> io::Error {
    io::Error::new(io::ErrorKind::TimedOut, "previous version did not exit")
}

#[cfg(not(any(target_os = "macos", windows)))]
fn wait_for_exit(_pid: u32, _timeout: Duration) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "unsupported update platform",
    ))
}

#[cfg(test)]
#[path = "install_tests.rs"]
pub(crate) mod tests;
