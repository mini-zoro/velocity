//! Recovery for an in-place save. An inode lock serializes our writers and replayers.
//! The durable recovery record precedes any visible data changes.
mod proof;
mod retained;
mod stages;
#[cfg(target_os = "macos")]
mod undo;
mod volume;
use crate::{
    ContentIdentity,
    atomic::{self, Durability, Stage},
    regular,
};
use proof::Original;
use serde::{Deserialize, Serialize};
pub(crate) use stages::StageReference;
pub(super) use stages::{clean_stages, tracked_stage};
use std::{
    fs::{self, File, Metadata},
    io::{self, Read, Write},
    os::unix::{
        ffi::OsStrExt,
        fs::{DirBuilderExt, FileExt, MetadataExt, PermissionsExt},
    },
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime},
};

const STAGE: &str = ".velocity-save-";
const PRIVATE_STAGE: &str = ".velocity-save-data-";
const PROOF: &str = ".velocity-save-proof-";
const BACKUP: &str = ".velocity-rollback-";
const JOURNAL_TEMP: &str = ".velocity-journal-";
const REFERENCE_TEMP: &str = ".velocity-stage-ref-";
const MAX_BACKUP_COPY: u64 = 1024 * 1024;
const MAX_RECORD: u64 = 128 * 1024;

fn recovery_dir(profile: Option<&Path>) -> PathBuf {
    profile.map_or_else(
        || {
            std::env::temp_dir().join(format!(
                "velocity-save-recovery-{}",
                rustix::process::geteuid().as_raw()
            ))
        },
        |path| path.join("Recovery"),
    )
}

fn check_directory(root: &Path) -> io::Result<()> {
    let m = fs::symlink_metadata(root)?;
    if !m.is_dir() || m.uid() != rustix::process::geteuid().as_raw() || m.mode() & 0o077 != 0 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "save recovery requires a private, user-owned directory",
        ));
    }
    Ok(())
}

fn ensure_directory(root: &Path) -> io::Result<()> {
    match check_directory(root) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            fs::create_dir_all(atomic::parent(root))?;
            match fs::DirBuilder::new().mode(0o700).create(root) {
                Ok(()) => atomic::sync_parent(root)?,
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => (),
                Err(e) => return Err(e),
            }
            check_directory(root)
        }
        result => result,
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
struct Entry {
    device: u64,
    inode: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    volume: Option<[u8; 16]>,
}

impl Entry {
    fn of(m: &Metadata) -> Self {
        Self {
            device: m.dev(),
            inode: m.ino(),
            volume: None,
        }
    }

    fn on_volume(m: &Metadata, volume: &volume::Volume) -> Self {
        Self {
            volume: volume.uuid,
            ..Self::of(m)
        }
    }

    fn same_volume(self, other: Self) -> bool {
        match self.volume {
            Some(uuid) => other.volume == Some(uuid),
            None => self.device == other.device,
        }
    }

    fn same_file(self, other: Self) -> bool {
        self.inode == other.inode && self.same_volume(other)
    }

    fn mount_key(self) -> String {
        self.volume.map_or_else(
            || format!("{:016x}", self.device),
            |uuid| format!("{:032x}", u128::from_be_bytes(uuid)),
        )
    }

    fn record_name(self) -> String {
        format!("{}-{:016x}.save-journal", self.mount_key(), self.inode)
    }
}

// Unix paths may not be UTF-8. Serialize the exact pathname, not a lossy display name.
#[derive(Debug, Deserialize, Serialize)]
struct JournalPath(Vec<u8>);

impl JournalPath {
    fn new(path: &Path) -> Self {
        Self(path.as_os_str().as_bytes().to_vec())
    }

    fn get(&self) -> &Path {
        Path::new(std::ffi::OsStr::from_bytes(&self.0))
    }
}

#[derive(Debug, Deserialize, Serialize)]
struct Journal {
    version: u8,
    target: JournalPath,
    target_entry: Entry,
    stage: JournalPath,
    stage_entry: Entry,
    stage_len: u64,
    #[serde(default)]
    content: Option<ContentIdentity>,
    rollback: Option<JournalPath>,
    mode: u32,
    #[serde(default)]
    original_len: Option<u64>,
    #[serde(default)]
    birth: Option<SystemTime>,
    #[serde(default)]
    synthetic_empty_id: bool,
    #[serde(default)]
    original: Option<Original>,
}

pub(super) struct Transaction<'a> {
    target: &'a Path,
    root: PathBuf,
    record: PathBuf,
    entry: Entry,
    volume: volume::Volume,
    pub(super) recovery_notice: Option<String>,
    _profile_lock: Option<File>,
}

pub(super) fn begin<'a>(
    target: &'a Path,
    file: &File,
    expected: &mut Metadata,
    profile: Option<&Path>,
    expected_disk: Option<ContentIdentity>,
) -> Result<Transaction<'a>, String> {
    let root = std::path::absolute(recovery_dir(profile)).map_err(|e| regular::error(target, e))?;
    let volume = volume::inspect(file);
    let entry = Entry::of(expected);
    let mut baseline = expected_disk;
    // A library caller without a previously accepted revision starts at
    // lock acquisition. Only snapshot before the wait if another writer
    // has the inode. The uncontended staged path reads the original once.
    let mut contended = false;
    let profile_lock = lock_with(file, target, &root, || {
        if !contended && baseline.is_none() {
            baseline = Some(disk_revision(target, entry).map_err(io::Error::other)?);
        }
        contended = true;
        fault(Phase::Waiting)
    })?;
    check_entry(target, file, entry).map_err(|_| crate::save::changed(target))?;
    // Recovery can legitimately change the contents at this point. Check the
    // revision we agreed to overwrite before replaying any pending write.
    if (contended || volume.coarse_timestamps)
        && let Some(baseline) = baseline
        && (file
            .metadata()
            .map_err(|e| regular::error(target, e))?
            .len()
            != baseline.bytes
            || disk_revision(target, entry)? != baseline)
    {
        return Err(crate::save::changed(target));
    }
    let (notice, verified) = replay_pending(&root, target, file, expected, &volume)?;
    if verified {
        *expected = file.metadata().map_err(|e| regular::error(target, e))?;
    }
    let entry = Entry::on_volume(expected, &volume);
    Ok(Transaction {
        target,
        record: root.join(entry.record_name()),
        root,
        entry,
        volume,
        recovery_notice: notice,
        _profile_lock: profile_lock,
    })
}

fn disk_revision(path: &Path, entry: Entry) -> Result<ContentIdentity, String> {
    #[cfg(test)]
    fault(Phase::DiskDigest).map_err(|e| regular::error(path, e))?;
    let file = regular::read(path)?;
    let error = |e| regular::error(path, e);
    let before = file.metadata().map_err(error)?;
    if Entry::of(&before) != entry {
        return Err(crate::save::changed(path));
    }
    let digest = proof::checksum(&file, before.len()).map_err(error)?;
    if !crate::save::same_version(&before, &file.metadata().map_err(error)?) {
        return Err(crate::save::changed(path));
    }
    Ok(digest)
}

#[cfg(test)]
thread_local! {
    pub(super) static COARSE_VERSION_PROBE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn pending_records(
    root: &Path,
    target: &Path,
    metadata: &Metadata,
    volume: &volume::Volume,
) -> Result<Vec<PathBuf>, String> {
    match check_directory(root) {
        Ok(()) => (),
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(regular::error(root, e)),
    }
    let entry = Entry::on_volume(metadata, volume);
    let old_key = Entry::of(metadata);
    let mut paths = Vec::new();
    for key in [entry, old_key]
        .into_iter()
        .take(if entry == old_key { 1 } else { 2 })
    {
        let path = root.join(key.record_name());
        if pending_file(&path)? {
            paths.push(path);
        }
        #[cfg(target_os = "macos")]
        for name in [undo::name(key), undo::inline::name(key)] {
            let path = root.join(name);
            if pending_file(&path)? {
                paths.push(path);
            }
        }
    }
    // A first write to an empty FAT/exFAT file can change its synthetic ID.
    if volume.synthetic_empty_id
        && let Ok(records) = fs::read_dir(root)
    {
        for path in records.flatten().map(|entry| entry.path()) {
            if path.extension().is_some_and(|e| e == "save-journal")
                && !paths.contains(&path)
                && let Ok((_, journal)) = read_journal(root, &path)
                && journal.synthetic_empty_id
                && journal.target.get() == target
            {
                paths.push(path);
            }
        }
    }
    Ok(paths)
}

fn replay_pending(
    root: &Path,
    target: &Path,
    file: &File,
    metadata: &Metadata,
    volume: &volume::Volume,
) -> Result<(Option<String>, bool), String> {
    let mut notices = Vec::new();
    let mut verified = false;
    for path in pending_records(root, target, metadata, volume)? {
        let action = recover_record(root, &path, Some(file))?;
        verified |= action.verified;
        notices.extend(action.message);
    }
    Ok(((!notices.is_empty()).then(|| notices.join("; ")), verified))
}

/// Verified disk revision and diagnostic from replaying a pending save.
#[derive(Debug)]
pub struct RecoveredSave {
    /// Raw checksum from the inode under its lock. None when only cleanup ran.
    pub content: Option<ContentIdentity>,
    /// Non-modal recovery message.
    pub notice: String,
}

/// Replays only a pending save for this file. Call on a save conflict before
/// asking for overwrite permission; any unrelated edit still needs consent.
pub fn recover_pending_save(
    path: &Path,
    profile: Option<&Path>,
) -> Result<Option<RecoveredSave>, String> {
    let io_error = |e| regular::error(path, e);
    let root = std::path::absolute(recovery_dir(profile)).map_err(io_error)?;
    match check_directory(&root) {
        Ok(()) => (),
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(io_error(e)),
    }
    let target = fs::canonicalize(path).map_err(io_error)?;
    let readable = regular::read(&target)?;
    let opened = readable.metadata().map_err(io_error)?;
    let volume = volume::inspect(&readable);
    if pending_records(&root, &target, &opened, &volume)?.is_empty() {
        return Ok(None);
    }
    let writable = regular::writable(&target)?;
    let _lock = lock(&writable, &target, &root)?;
    check_entry(&target, &writable, Entry::of(&opened))?;
    let current = writable.metadata().map_err(io_error)?;
    let (notice, verified) = replay_pending(&root, &target, &writable, &current, &volume)?;
    if notice.is_none() && !verified {
        // An active writer may have finished while we waited for its lock.
        return Ok(None);
    }
    let content = if verified {
        let current = writable.metadata().map_err(io_error)?;
        check_entry(&target, &readable, Entry::of(&current))?;
        Some(proof::checksum(&readable, current.len()).map_err(io_error)?)
    } else {
        None
    };
    Ok(Some(RecoveredSave {
        content,
        notice: notice.unwrap_or_default(),
    }))
}

fn pending_file(path: &Path) -> Result<bool, String> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(regular::error(path, e)),
    }
}

fn wait_for_lock(file: &File, mut try_lock: impl FnMut() -> io::Result<()>) -> io::Result<()> {
    wait_for_lock_with(file, &mut try_lock, || Ok(()))
}

fn wait_for_lock_with(
    file: &File,
    mut try_lock: impl FnMut() -> io::Result<()>,
    mut waiting: impl FnMut() -> io::Result<()>,
) -> io::Result<()> {
    use rustix::io::{FdFlags, fcntl_getfd};
    debug_assert!(fcntl_getfd(file).is_ok_and(|flags| flags.contains(FdFlags::CLOEXEC)));
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut backoff = Duration::from_millis(2);
    let mut notified = false;
    loop {
        match try_lock() {
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                ) =>
            {
                if !notified && e.kind() == io::ErrorKind::WouldBlock {
                    waiting()?;
                    notified = true;
                }
                let wait = deadline.saturating_duration_since(Instant::now());
                if wait.is_zero() {
                    return Err(e);
                }
                std::thread::sleep(wait.min(backoff));
                backoff = (backoff * 2).min(Duration::from_millis(50));
            }
            result => return result,
        }
    }
}

fn lock(file: &File, target: &Path, root: &Path) -> Result<Option<File>, String> {
    lock_with(file, target, root, || Ok(()))
}

fn lock_with(
    file: &File,
    target: &Path,
    root: &Path,
    mut waiting: impl FnMut() -> io::Result<()>,
) -> Result<Option<File>, String> {
    let error = |e| regular::error(target, format!("could not lock for save: {e}"));
    // CLOEXEC closes the inherited copy at exec; a fork may briefly keep a
    // previous writer's lock alive. A bounded wait also handles real contention.
    match wait_for_lock_with(
        file,
        || fault(Phase::Lock).and_then(|()| file.try_lock().map_err(Into::into)),
        &mut waiting,
    ) {
        Ok(()) => Ok(None),
        Err(e) if atomic::unsupported(&e) => {
            use std::os::unix::fs::OpenOptionsExt;
            ensure_directory(root).map_err(error)?;
            let lock = fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(false)
                .custom_flags(
                    (rustix::fs::OFlags::NOFOLLOW
                        | rustix::fs::OFlags::NONBLOCK
                        | rustix::fs::OFlags::CLOEXEC)
                        .bits() as i32,
                )
                .mode(0o600)
                .open(root.join(".save-lock"))
                .map_err(error)?;
            if !lock.metadata().map_err(error)?.is_file() {
                return Err(regular::error(target, "save lock is not a regular file"));
            }
            wait_for_lock_with(&lock, || lock.try_lock().map_err(Into::into), waiting)
                .map_err(error)?;
            Ok(Some(lock))
        }
        Err(e) => Err(error(e)),
    }
}

impl Transaction<'_> {
    pub(super) fn sibling_stage(&self) -> io::Result<Stage> {
        stages::new(
            &self.root,
            atomic::parent(self.target),
            None,
            Some(self.entry),
        )
    }

    pub(super) fn stage(&self) -> io::Result<Stage> {
        // Private preparations can be found by scanning Recovery itself.
        // Sibling stages need a separate pointer so startup can find them.
        ensure_directory(&self.root)?;
        fault(Phase::BeforeStage)?;
        let stage = Stage::new(&self.root, &preparation_prefix(PRIVATE_STAGE), None)?;
        fault(Phase::AfterStage)?;
        Ok(stage)
    }

    pub(super) fn write(
        &self,
        mut stage: Stage,
        target: &File,
        source: Option<&File>,
        original: &Metadata,
        content: ContentIdentity,
    ) -> Result<Option<String>, String> {
        let io_error = |e| regular::error(self.target, e);
        ensure_directory(&self.root).map_err(io_error)?;
        if stage.path().parent() != Some(&self.root)
            && let Err(e) = atomic::sync_parent(stage.path())
        {
            if !atomic::unsupported(&e) && e.kind() != io::ErrorKind::PermissionDenied {
                return Err(io_error(e));
            }
            // Directory sync isn't available on some volumes. Keep the recoverable
            // bytes and their name on the profile filesystem in that case.
            let mut local = self.stage().map_err(io_error)?;
            copy_data(
                stage.descriptor(),
                local.file(),
                stage.descriptor().metadata().map_err(io_error)?.len(),
                false,
            )
            .map_err(io_error)?;
            stage = local;
        }
        let staged = stage.descriptor().metadata().map_err(io_error)?;
        // The final target flush covers ordered writes on its device, but not
        // recovery data on another device. Flush those bytes independently.
        let recovery_device = fs::metadata(&self.root).map_err(io_error)?.dev();
        let durability = recovery_durability(self.entry.device, recovery_device);
        stage
            .sync(if staged.dev() == recovery_device {
                // The following journal flush orders both files here.
                Durability::Cached
            } else {
                Durability::Barrier
            })
            .map_err(|e| stage_error(self.target, e))?;
        crate::save::check_target(self.target, original)?;
        let extend = reserve(
            target,
            original.len(),
            staged.len(),
            self.volume.synthetic_empty_id,
        )
        .map_err(|e| stage_error(self.target, e))?;
        let mut rollback = source.and_then(|file| backup(file, &self.root, original.len()));
        let (original_proof, mut proof_stage) = match source {
            Some(file) => {
                let (proof, stage) =
                    Original::capture(file, original.len(), staged.len(), &self.root)
                        .map_err(io_error)?;
                (Some(proof), stage)
            }
            None => (None, None),
        };
        check_entry(self.target, target, self.entry)?;

        let journal = Journal {
            version: 3,
            target: JournalPath::new(self.target),
            target_entry: self.entry,
            stage: JournalPath::new(stage.path()),
            stage_entry: Entry::on_volume(&staged, &volume::inspect(stage.descriptor())),
            stage_len: staged.len(),
            content: Some(content),
            rollback: rollback.as_ref().map(|path| JournalPath::new(path)),
            mode: original.mode() & 0o7777,
            original_len: Some(original.len()),
            birth: original.created().ok(),
            synthetic_empty_id: self.volume.synthetic_empty_id,
            original: original_proof,
        };
        let mut record =
            Stage::new(&self.root, &preparation_prefix(JOURNAL_TEMP), None).map_err(io_error)?;
        // The cleaner never removes a preparation that a writer still holds.
        let record_lock = record.descriptor().try_clone().map_err(io_error)?;
        wait_for_lock(&record_lock, || record_lock.try_lock().map_err(Into::into))
            .map_err(io_error)?;
        fault(Phase::Journal).map_err(io_error)?;
        write_record(record.file(), &journal).map_err(io_error)?;
        let published = record.publish(&self.record, true).map_err(io_error)?;
        stage.retain_for_recovery();
        if let Some(stage) = &mut proof_stage {
            stage.retain_for_recovery();
        }
        if let Some(path) = &mut rollback {
            path.disable_cleanup(true);
        }
        if let Some(e) = published.warning {
            return Err(pending(self.target, &self.record, &e));
        }
        // Order the record contents and published entry before we overwrite.
        // A separate profile device needs its own full flush; no target flush
        // can persist a journal across devices.
        let visible =
            regular::read(&self.record).map_err(|e| pending(self.target, &self.record, &e))?;
        atomic::sync(&visible, durability).map_err(|e| pending(self.target, &self.record, &e))?;

        let result = if extend {
            fault(Phase::Extend).and_then(|()| target.set_len(staged.len()))
        } else {
            Ok(())
        };
        if let Err(e) = result {
            // Failed extension occurs before we overwrite any bytes.
            if target.metadata().is_ok_and(|m| m.len() == original.len()) {
                return match discard(&self.record, &journal, true) {
                    Ok(()) => Err(io_error(e)),
                    Err(cleanup) => Err(pending(
                        self.target,
                        &self.record,
                        &format!("{e}; cleanup failed: {cleanup}"),
                    )),
                };
            }
            return Err(pending(self.target, &self.record, &e));
        }

        // Extension on unsupported filesystems is journaled and verified too.
        check_entry(self.target, target, self.entry)
            .map_err(|e| pending(self.target, &self.record, &e))?;
        match write_revision(stage.descriptor(), target, &journal, true) {
            Ok(warning) => Ok(discard(&self.record, &journal, false)
                .err()
                .map(|e| {
                    pending(
                        self.target,
                        &self.record,
                        &format!("data saved; cleanup failed: {e}"),
                    )
                })
                .or(warning)),
            Err(e) => {
                if let Some(path) = &rollback
                    && restore(path, target, stage.descriptor(), &journal, original).is_ok()
                {
                    return match discard(&self.record, &journal, true) {
                        Ok(()) => Err(regular::error(
                            self.target,
                            format!("{e}; restored the original file"),
                        )),
                        Err(cleanup) => Err(pending(
                            self.target,
                            &self.record,
                            &format!("{e}; restored the original, but cleanup failed: {cleanup}"),
                        )),
                    };
                }
                Err(pending(
                    self.target,
                    &self.record,
                    &format!(
                        "{e}; complete staged content at {}",
                        journal.stage.get().display()
                    ),
                ))
            }
        }
    }
}

fn check_entry(path: &Path, file: &File, expected: Entry) -> Result<(), String> {
    let matches =
        |m: Metadata| m.is_file() && m.dev() == expected.device && m.ino() == expected.inode;
    if fs::symlink_metadata(path).is_ok_and(matches) && file.metadata().is_ok_and(matches) {
        Ok(())
    } else {
        Err(regular::error(
            path,
            "file identity changed; no data was overwritten",
        ))
    }
}

fn check_recovery_revision(path: &Path, file: &File, expected: &Metadata) -> Result<(), String> {
    check_entry(path, file, Entry::of(expected))?;
    let current = file.metadata().map_err(|e| regular::error(path, e))?;
    if !crate::save::same_version(expected, &current) {
        return Err(regular::error(
            path,
            "file changed while verifying recovery",
        ));
    }
    Ok(())
}

fn preparation_prefix(prefix: &str) -> String {
    format!("{prefix}{}-", std::process::id())
}

fn write_record(output: &mut impl Write, record: &impl Serialize) -> io::Result<()> {
    let bytes = serde_json::to_vec(record)?;
    if bytes.len() as u64 > MAX_RECORD {
        return Err(io::Error::other("save journal is too large"));
    }
    output.write_all(&bytes)
}

fn recovery_durability(target_device: u64, recovery_device: u64) -> Durability {
    #[cfg(test)]
    if CROSS_VOLUME_PROBE.get() {
        return Durability::Full;
    }
    if target_device == recovery_device {
        Durability::Barrier
    } else {
        Durability::Full
    }
}

#[cfg(test)]
thread_local! {
    static CROSS_VOLUME_PROBE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn pending(target: &Path, record: &Path, error: &dyn std::fmt::Display) -> String {
    regular::error(
        target,
        format!("{error}; save recovery: {}", record.display()),
    )
}

fn reserve(file: &File, old_len: u64, new_len: u64, synthetic_empty_id: bool) -> io::Result<bool> {
    fault(Phase::Reserve)?;
    // FSKit's FAT drivers can assign a new synthetic inode (and damage later
    // writes) merely by preallocating an empty file. Neither reserve operation
    // is essential there: the complete, verified stage is already durable.
    if new_len <= old_len || old_len == 0 || synthetic_empty_id {
        return Ok(false);
    }
    match fault(Phase::Preallocate).and_then(|()| preallocate(file, old_len, new_len)) {
        Ok(()) => Ok(false),
        Err(e) if atomic::unsupported(&e) => Ok(true), // Extend only AFTER publishing the journal.
        Err(e) => Err(e),
    }
}

#[cfg(unix)]
pub(super) fn stage_error(path: &Path, error: io::Error) -> String {
    use rustix::io::Errno;
    if matches!(error.raw_os_error(), Some(e) if e == Errno::NOSPC.raw_os_error() || e == Errno::DQUOT.raw_os_error())
    {
        regular::error(
            path,
            format!(
                "{error}; this volume needs space for a complete staged revision to preserve the original. Free space and retry"
            ),
        )
    } else {
        regular::error(path, error)
    }
}

pub(super) fn out_of_space(error: &str) -> bool {
    [rustix::io::Errno::NOSPC, rustix::io::Errno::DQUOT]
        .iter()
        .any(|code| error.ends_with(&format!("(os error {})", code.raw_os_error())))
}

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
fn preallocate(file: &File, old_len: u64, new_len: u64) -> io::Result<()> {
    use std::os::fd::AsRawFd;
    // https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/fcntl.2.html
    let mut store = libc::fstore_t {
        fst_flags: libc::F_ALLOCATEALL,
        fst_posmode: libc::F_PEOFPOSMODE,
        fst_offset: 0,
        fst_length: i64::try_from(new_len - old_len).map_err(io::Error::other)?,
        fst_bytesalloc: 0,
    };
    loop {
        // SAFETY: F_PREALLOCATE accepts a mutable fstore_t on a live regular file.
        if unsafe { libc::fcntl(file.as_raw_fd(), libc::F_PREALLOCATE, &raw mut store) } == 0 {
            return Ok(());
        }
        let e = io::Error::last_os_error();
        if e.kind() != io::ErrorKind::Interrupted {
            return Err(e);
        }
    }
}

#[cfg(target_os = "linux")]
fn preallocate(file: &File, old_len: u64, new_len: u64) -> io::Result<()> {
    use rustix::fs::{FallocateFlags, fallocate};
    fallocate(file, FallocateFlags::KEEP_SIZE, old_len, new_len - old_len).map_err(Into::into)
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn preallocate(_file: &File, _old_len: u64, _new_len: u64) -> io::Result<()> {
    Err(rustix::io::Errno::NOTSUP.into())
}

fn backup(source: &File, root: &Path, len: u64) -> Option<tempfile::TempPath> {
    fault(Phase::Backup).ok()?;
    let mut builder = tempfile::Builder::new();
    let prefix = preparation_prefix(BACKUP);
    builder.prefix(&prefix);
    #[cfg(target_os = "macos")]
    if let Ok(dir) = File::open(root)
        && let Ok(clone) = builder.make_in(root, |path| {
            // APFS clones share blocks until either file is written; an overwrite
            // can still run out of space. Never treat the clone as preallocation.
            // https://github.com/apple-oss-distributions/xnu/blob/main/bsd/man/man2/clonefile.2
            rustix::fs::fclonefileat(source, &dir, path, rustix::fs::CloneFlags::NOOWNERCOPY)
                .map_err(io::Error::from)
        })
    {
        return Some(clone.into_temp_path());
    }
    if len > MAX_BACKUP_COPY {
        return None;
    }
    let mut copy = builder.tempfile_in(root).ok()?;
    copy_data(source, copy.as_file_mut(), len, false).ok()?;
    atomic::sync(copy.as_file(), Durability::Barrier).ok()?;
    Some(copy.into_temp_path())
}

fn mode_after_write(target: &File, mode: u32) -> io::Result<()> {
    if target.metadata()?.mode() & 0o7777 != mode {
        // Some Unix filesystems clear set-id bits during writes.
        target.set_permissions(fs::Permissions::from_mode(mode))?;
    }
    Ok(())
}

fn copy_data(source: &File, target: &File, len: u64, inject: bool) -> io::Result<()> {
    let mut chunk = vec![0; len.min(1024 * 1024) as usize];
    let mut offset = 0;
    while offset < len {
        let count = (len - offset).min(chunk.len() as u64) as usize;
        source.read_exact_at(&mut chunk[..count], offset)?;
        #[cfg(test)]
        if inject && offset == 0 {
            let n = count.div_ceil(2).min(4096);
            target.write_all_at(&chunk[..n], 0)?;
            fault(Phase::Write)?;
        }
        let _ = inject;
        target.write_all_at(&chunk[..count], offset)?;
        offset += count as u64;
    }
    Ok(())
}

fn preserve_mode(target: &File, path: &Path, mode: u32) -> Option<String> {
    mode_after_write(target, mode).err().map(|e| {
        regular::error(
            path,
            format!("saved, but could not preserve permissions: {e}"),
        )
    })
}

fn write_revision(
    source: &File,
    target: &File,
    journal: &Journal,
    inject: bool,
) -> io::Result<Option<String>> {
    #[cfg(test)]
    if inject {
        fault(Phase::BeforeWrite)?;
    }
    copy_data(source, target, journal.stage_len, inject)?;
    if inject {
        fault(Phase::Truncate)?;
    }
    target.set_len(journal.stage_len)?;
    let warning = preserve_mode(target, journal.target.get(), journal.mode);
    if inject {
        fault(Phase::SyncTarget)?;
    }
    atomic::sync(target, Durability::Full)?;
    #[cfg(test)]
    if inject {
        fault(Phase::AfterSync)?;
    }
    Ok(warning)
}

fn restore(
    path: &Path,
    target: &File,
    stage: &File,
    journal: &Journal,
    original: &Metadata,
) -> Result<(), String> {
    let error = |e| regular::error(path, e);
    let rollback = regular::read(path)?;
    let readable = regular::read(journal.target.get())?;
    let actual = target.metadata().map_err(error)?;
    check_entry(journal.target.get(), &readable, Entry::of(&actual))?;
    let old = proof::Revision::file(&rollback, 0, original.len());
    let new = proof::Revision::file(stage, 0, journal.stage_len);
    if matches!(
        proof::verify(&readable, actual.len(), old, Some(new)).map_err(error)?,
        proof::Overwrite::Foreign
    ) {
        return Err(error(io::Error::other(
            "could not prove the interrupted target revision",
        )));
    }
    check_recovery_revision(journal.target.get(), target, &actual)?;
    copy_data(&rollback, target, original.len(), false).map_err(error)?;
    target.set_len(original.len()).map_err(error)?;
    mode_after_write(target, journal.mode).map_err(error)?;
    if let Ok(time) = original.modified() {
        let _ = target.set_modified(time);
    }
    atomic::sync(target, Durability::Full).map_err(error)
}

fn discard(path: &Path, journal: &Journal, sync_removal: bool) -> io::Result<()> {
    fs::remove_file(path)?;
    // Only a successful target write is safe to recognize by its checksum on
    // relaunch without prior durable removal of the journal.
    if sync_removal {
        atomic::sync_parent(path)?;
    }
    for artifact in [
        Some(journal.stage.get()),
        journal.rollback.as_ref().map(JournalPath::get),
        journal
            .original
            .as_ref()
            .and_then(Original::pages)
            .map(proof::Pages::path),
    ]
    .into_iter()
    .flatten()
    {
        match fs::remove_file(artifact) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => (),
            result => result?,
        }
    }
    stages::remove_companion(journal.stage.get()).map_err(io::Error::other)?;
    stages::forget(atomic::parent(path), journal.stage.get())?;
    Ok(())
}

/// Replays interrupted saves before reading restored documents. A replaced
/// target's staged revision is kept separately; unavailable files are retried.
pub fn recover_saves(profile: Option<&Path>) -> Vec<String> {
    let root = match std::path::absolute(recovery_dir(profile)) {
        Ok(root) => root,
        Err(e) => return vec![format!("Save recovery: {e}")],
    };
    match check_directory(&root) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Vec::new(),
        Err(e) => return vec![regular::error(&root, e)],
        Ok(()) => (),
    }
    let entries = match fs::read_dir(&root) {
        Ok(entries) => entries,
        Err(e) => return vec![regular::error(&root, e)],
    };
    let mut messages: Vec<_> = entries
        .filter_map(|entry| match entry {
            // AppleDouble companions (`._name`) appear next to every file on FAT,
            // exFAT and SMB volumes; they are never recovery records.
            Ok(entry)
                if !entry.file_name().as_encoded_bytes().starts_with(b"._")
                    && !staged_name(&entry.path(), retained::PREFIX)
                    && entry.path().extension().is_some_and(|ext| {
                        ext == "save-journal"
                            || (cfg!(target_os = "macos")
                                && (ext == "save-undo" || ext == "save-small"))
                    }) =>
            {
                match recover_record(&root, &entry.path(), None) {
                    Ok(action) => action.message,
                    Err(error) => Some(error),
                }
            }
            Err(e) => Some(regular::error(&root, e)),
            _ => None,
        })
        .collect();
    messages.extend(stages::clean(&root, None));
    messages
}

struct RecoveryAction {
    message: Option<String>,
    verified: bool,
}

fn recover_record(root: &Path, path: &Path, held: Option<&File>) -> Result<RecoveryAction, String> {
    #[cfg(target_os = "macos")]
    if path
        .extension()
        .is_some_and(|e| e == "save-undo" || e == "save-small")
    {
        return undo::recover(path, held);
    }
    recover(root, path, held)
}

fn staged_name(path: &Path, prefix: &str) -> bool {
    path.file_name()
        .is_some_and(|name| name.as_encoded_bytes().starts_with(prefix.as_bytes()))
}

fn read_journal(root: &Path, path: &Path) -> Result<(Entry, Journal), String> {
    let error = |e| regular::error(path, e);
    let record = regular::read(path)?;
    let metadata = record.metadata().map_err(error)?;
    if metadata.len() > MAX_RECORD {
        return Err(regular::error(path, "oversized save journal; retained"));
    }
    let record_entry = Entry::of(&metadata);
    let journal: Journal = serde_json::from_reader((&record).take(MAX_RECORD))
        .map_err(|e| regular::error(path, format!("could not read save journal: {e}")))?;
    let target = journal.target.get();
    let stage = journal.stage.get();
    if !matches!(journal.version, 1..=3)
        || !path
            .file_name()
            .is_some_and(|name| name == journal.target_entry.record_name().as_str())
        || !target.is_absolute()
        || !stage.is_absolute()
        || !staged_name(stage, STAGE)
        || journal.rollback.as_ref().is_some_and(|rollback| {
            rollback.get().parent() != Some(root) || !staged_name(rollback.get(), BACKUP)
        })
        || journal
            .original
            .as_ref()
            .and_then(Original::pages)
            .is_some_and(|p| !p.valid_in(root))
    {
        return Err(regular::error(
            path,
            "invalid save journal; retained for manual recovery",
        ));
    }
    Ok((record_entry, journal))
}

fn recover(root: &Path, path: &Path, held: Option<&File>) -> Result<RecoveryAction, String> {
    let (record_entry, journal) = read_journal(root, path)?;
    let target = journal.target.get();
    let stage = journal.stage.get();
    let remainder = |e: &dyn std::fmt::Display| {
        pending(
            target,
            path,
            &format!(
                "save not recovered ({e}); complete staged content at {}",
                stage.display()
            ),
        )
    };
    let owned;
    let _lock;
    let destination = if let Some(held) = held {
        held
    } else {
        owned = match regular::read(target) {
            Ok(file) => file,
            Err(_) if fs::symlink_metadata(target).is_ok_and(|m| !m.is_file()) => {
                return retained::archive(root, path, &journal, "file identity changed")
                    .map_err(|e| remainder(&e));
            }
            Err(error) => return Err(remainder(&error)),
        };
        _lock = lock(&owned, target, root).map_err(|e| remainder(&e))?;
        &owned
    };
    // The writer could have completed between enumeration and acquiring the lock.
    if !fs::symlink_metadata(path).is_ok_and(|m| m.is_file() && Entry::of(&m) == record_entry) {
        return Ok(RecoveryAction {
            message: Some(format!(
                "{}: save recovery record changed; no data replayed",
                target.display()
            )),
            verified: false,
        });
    }
    let now = destination.metadata().map_err(|e| remainder(&e))?;
    let volume = volume::inspect(destination);
    let current = Entry::on_volume(&now, &volume);
    check_entry(target, destination, current).map_err(|e| remainder(&e))?;
    let same_birth = journal
        .birth
        .is_none_or(|birth| now.created().ok() == Some(birth));
    let synthetic = journal.synthetic_empty_id
        && volume.synthetic_empty_id
        && journal.birth.is_some()
        && same_birth
        && journal.target_entry.same_volume(current);
    if !(same_birth && journal.target_entry.same_file(current) || synthetic) {
        return retained::archive(root, path, &journal, "file identity changed")
            .map_err(|e| remainder(&e));
    }
    let readable = regular::read(target)
        .ok()
        .filter(|file| check_entry(target, file, current).is_ok());
    if let (Some(content), Some(readable)) = (&journal.content, &readable)
        && now.len() == content.bytes
        && proof::checksum(readable, now.len()).is_ok_and(|actual| &actual == content)
    {
        check_recovery_revision(target, destination, &now).map_err(|e| remainder(&e))?;
        atomic::sync(destination, Durability::Full).map_err(|e| remainder(&e))?;
        discard(path, &journal, false).map_err(|e| remainder(&e))?;
        return Ok(RecoveryAction {
            message: (!synthetic || journal.target_entry.same_file(current))
                .then(|| format!("Completed save already intact: {}", target.display())),
            verified: true,
        });
    }
    if regular::metadata(stage, true)?.is_none() {
        // An earlier recovery may have moved the stage into Recovery, then
        // died before archiving the original or retiring the record.
        return retained::archive(root, path, &journal, "staged revision already archived")
            .map_err(|e| remainder(&e));
    }
    let source =
        retained::open_verified(stage, &journal, journal.content).map_err(|e| remainder(&e))?;
    if synthetic && !journal.target_entry.same_file(current) {
        // No extension was reserved on FAT/exFAT: the entire observed file
        // must be an exact staged prefix to accept a changed synthetic ID.
        let Some(readable) = &readable else {
            return Err(remainder(
                &"could not check the target against the staged prefix",
            ));
        };
        if journal.original_len != Some(0)
            || !proof::is_prefix(readable, &source, now.len(), journal.stage_len)
                .map_err(|e| remainder(&e))?
        {
            return retained::archive(root, path, &journal, "file identity changed")
                .map_err(|e| remainder(&e));
        }
    } else {
        let verified = match (&journal.original, journal.original_len, &readable) {
            (Some(original), Some(old_len), Some(readable)) => original
                .matches(readable, &source, now.len(), old_len, journal.stage_len)
                .unwrap_or(false),
            _ => false,
        };
        if !verified {
            return retained::archive(
                root,
                path,
                &journal,
                "file changed since the interrupted save",
            )
            .map_err(|e| remainder(&e));
        }
    }
    check_recovery_revision(target, destination, &now).map_err(|e| remainder(&e))?;
    let opened;
    let destination = if let Some(held) = held {
        held
    } else {
        // Open for write only after proving the observed revision. On macOS
        // this can itself change metadata on a compressed file.
        opened = regular::writable(target).map_err(|e| remainder(&e))?;
        check_recovery_revision(target, &opened, &now).map_err(|e| remainder(&e))?;
        &opened
    };
    let warning =
        write_revision(&source, destination, &journal, false).map_err(|e| remainder(&e))?;
    discard(path, &journal, journal.content.is_none()).map_err(|e| {
        pending(
            target,
            path,
            &format!("recovered data; cleanup failed: {e}"),
        )
    })?;
    Ok(RecoveryAction {
        message: Some(
            warning.unwrap_or_else(|| format!("Recovered interrupted save: {}", target.display())),
        ),
        verified: true,
    })
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Lock,
    Waiting,
    BeforeStage,
    AfterStage,
    Reserve,
    Preallocate,
    Backup,
    Journal,
    Extend,
    #[cfg(test)]
    BeforeWrite,
    #[cfg(test)]
    SmallCreated,
    #[cfg(test)]
    SmallPreparing,
    #[cfg(test)]
    Write,
    #[cfg(test)]
    Rollback,
    #[cfg(test)]
    RollbackTail,
    #[cfg(test)]
    RollbackWrite,
    Truncate,
    SyncTarget,
    ArchiveRename,
    ArchivePublished,
    #[cfg(test)]
    DiskDigest,
    #[cfg(test)]
    ClonePrepared,
    #[cfg(test)]
    AfterSync,
}

fn fault(phase: Phase) -> io::Result<()> {
    #[cfg(test)]
    if let Some(action) = FAULT.with(|slot| {
        slot.borrow()
            .iter()
            .find(|(p, _)| *p == phase)
            .map(|(_, f)| *f)
    }) {
        return action();
    }
    let _ = phase;
    Ok(())
}

#[cfg(test)]
fn has_fault(phase: Phase) -> bool {
    FAULT.with(|slot| slot.borrow().iter().any(|(p, _)| *p == phase))
}

#[cfg(test)]
type Fault = (Phase, fn() -> io::Result<()>);
#[cfg(test)]
thread_local! {
    static FAULT: std::cell::RefCell<Vec<Fault>> = const { std::cell::RefCell::new(Vec::new()) };
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use crate::{Format, save_report};
    use std::{os::unix::process::ExitStatusExt, process::Command};
    use vel_text::Text;
    mod native;
    mod race;

    pub(super) fn save(target: &Path, profile: &Path, text: &str) -> Result<crate::Saved, String> {
        save_report(
            target,
            &Text::from(text),
            &Format::default(),
            false,
            Some(profile),
        )
    }

    pub(super) fn use_faults<T>(faults: &[Fault], run: impl FnOnce() -> T) -> T {
        FAULT.with(|slot| slot.replace(faults.to_vec()));
        let result = run();
        FAULT.with(|slot| slot.borrow_mut().clear());
        result
    }

    fn disk_full() -> io::Result<()> {
        Err(io::Error::from_raw_os_error(libc::ENOSPC))
    }

    fn quota() -> io::Result<()> {
        Err(io::Error::from_raw_os_error(libc::EDQUOT))
    }

    pub(super) fn io_failure() -> io::Result<()> {
        Err(io::Error::from_raw_os_error(libc::EIO))
    }

    fn unsupported() -> io::Result<()> {
        Err(io::Error::from_raw_os_error(libc::ENOTSUP))
    }

    #[test]
    fn recovery_records_are_serialized_with_one_contiguous_write() {
        #[derive(Default)]
        struct Writes(Vec<Vec<u8>>);
        impl Write for Writes {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                self.0.push(bytes.to_vec());
                Ok(bytes.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }

        let path = Path::new("/example/document");
        let record = Journal {
            version: 2,
            target: JournalPath::new(path),
            target_entry: Entry {
                device: 1,
                inode: 2,
                volume: None,
            },
            stage: JournalPath::new(Path::new("/example/.velocity-save-test")),
            stage_entry: Entry {
                device: 1,
                inode: 3,
                volume: None,
            },
            stage_len: 4096,
            content: None,
            rollback: None,
            mode: 0o600,
            original_len: Some(4096),
            birth: None,
            synthetic_empty_id: false,
            original: Some(Original::Inline(vec![b'a'; 4096])),
        };
        let mut output = Writes::default();
        write_record(&mut output, &record).unwrap();
        assert_eq!(output.0.len(), 1);
        let reparsed: Journal = serde_json::from_slice(&output.0[0]).unwrap();
        assert_eq!(reparsed.target.get(), path);
    }

    #[test]
    fn enospc_quota_and_eio_restore_or_fail_before_mutating() {
        let profile = tempfile::tempdir().unwrap();
        let target = profile.path().join("target");
        let original = "old contents";
        fs::write(&target, original).unwrap();
        let ino = target.metadata().unwrap().ino();
        let new = "new text that exceeds the original length";
        for phase in [
            Phase::Reserve,
            Phase::Journal,
            Phase::Write,
            Phase::Truncate,
            Phase::SyncTarget,
        ] {
            for failure in [disk_full, quota, io_failure] {
                let result = use_faults(&[(phase, failure)], || save(&target, profile.path(), new));
                assert!(result.is_err(), "{result:?}");
                assert_eq!(fs::read_to_string(&target).unwrap(), original);
                assert_eq!(target.metadata().unwrap().ino(), ino);
                assert_eq!(
                    profile.path().join("Recovery").read_dir().unwrap().count(),
                    0
                );
            }
        }
        let result = use_faults(
            &[
                (Phase::Preallocate, unsupported),
                (Phase::Extend, disk_full),
            ],
            || save(&target, profile.path(), new),
        );
        assert!(result.is_err(), "{result:?}");
        assert_eq!(fs::read_to_string(&target).unwrap(), original);
        assert_eq!(
            profile.path().join("Recovery").read_dir().unwrap().count(),
            0
        );
        use_faults(&[(Phase::Preallocate, unsupported)], || {
            save(&target, profile.path(), new)
        })
        .unwrap();
        assert_eq!(fs::read_to_string(&target).unwrap(), new);
    }

    #[test]
    fn eio_without_rollback_reports_and_retains_the_full_stage() {
        let profile = tempfile::tempdir().unwrap();
        let target = profile.path().join("target");
        fs::write(&target, "old contents").unwrap();
        let new = "new revision whose first write will fail";
        let error = use_faults(
            &[(Phase::Write, io_failure), (Phase::Backup, unsupported)],
            || save(&target, profile.path(), new),
        )
        .unwrap_err();
        assert!(error.contains("complete staged content at"), "{error}");
        let root = profile.path().join("Recovery");
        let record = fs::read_dir(&root)
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .find(|p| p.extension().is_some_and(|ext| ext == "save-journal"))
            .unwrap();
        let journal: Journal = serde_json::from_reader(File::open(&record).unwrap()).unwrap();
        assert_eq!(fs::read_to_string(journal.stage.get()).unwrap(), new);
        let messages = recover_saves(Some(profile.path()));
        assert!(
            messages[0].starts_with("Recovered interrupted save:"),
            "{messages:?}"
        );
        assert_eq!(fs::read_to_string(&target).unwrap(), new);
        assert!(!journal.stage.get().exists());
        assert!(!record.exists());
    }

    #[test]
    fn filesystems_without_flock_use_a_private_profile_lock() {
        let profile = tempfile::tempdir().unwrap();
        let target = profile.path().join("target");
        fs::write(&target, "original").unwrap();
        use_faults(
            &[(Phase::Lock, unsupported), (Phase::Backup, unsupported)],
            || save(&target, profile.path(), "new revision"),
        )
        .unwrap();
        assert_eq!(fs::read_to_string(&target).unwrap(), "new revision");
        assert!(profile.path().join("Recovery/.save-lock").is_file());
        assert!(recover_saves(Some(profile.path())).is_empty());
    }

    #[test]
    fn save_waits_for_a_short_lived_cloexec_lock() {
        use rustix::io::{FdFlags, fcntl_getfd};
        let profile = tempfile::tempdir().unwrap();
        let target = profile.path().join("target");
        fs::write(&target, "original").unwrap();
        let held = regular::writable(&target).unwrap();
        assert!(fcntl_getfd(&held).unwrap().contains(FdFlags::CLOEXEC));
        held.try_lock().unwrap();
        // Release on the first actual lock attempt, after that attempt
        // observed the contention. No scheduling-dependent sleeps.
        use_faults(
            &[(Phase::Lock, || {
                TEST_LOCK.with(|lock| {
                    lock.take().map_or(Ok(()), |lock| {
                        lock.unlock()?;
                        Err(io::ErrorKind::WouldBlock.into())
                    })
                })
            })],
            || {
                TEST_LOCK.with(|lock| lock.replace(Some(held)));
                save(&target, profile.path(), "new revision").unwrap()
            },
        );
        assert_eq!(fs::read_to_string(&target).unwrap(), "new revision");
    }

    thread_local! {
        static TEST_LOCK: std::cell::Cell<Option<File>> = const { std::cell::Cell::new(None) };
    }

    #[test]
    fn empty_and_fat_targets_never_enter_preallocation() {
        let profile = tempfile::tempdir().unwrap();
        let target = profile.path().join("target");
        for (fat, old) in [(false, ""), (true, "original"), (true, "")] {
            fs::write(&target, old).unwrap();
            volume::FAT_PROBE.set(fat);
            use_faults(
                &[
                    (Phase::Backup, unsupported),
                    (Phase::Preallocate, || panic!("must not preallocate")),
                    (Phase::Extend, || panic!("must not extend before writing")),
                ],
                || save(&target, profile.path(), "new revision that needs space"),
            )
            .unwrap();
            volume::FAT_PROBE.set(false);
            assert_eq!(
                fs::read_to_string(&target).unwrap(),
                "new revision that needs space"
            );
            fs::write(&target, "external overwrite").unwrap();
            assert_eq!(fs::read_to_string(&target).unwrap(), "external overwrite");
        }
    }

    #[test]
    fn staged_save_reports_why_it_needs_free_space() {
        let profile = tempfile::tempdir().unwrap();
        let target = profile.path().join("target");
        fs::write(&target, "original").unwrap();
        let error = use_faults(
            &[(Phase::Backup, unsupported), (Phase::Reserve, disk_full)],
            || save(&target, profile.path(), "new revision").unwrap_err(),
        );
        assert!(
            error.contains("complete staged revision") && error.contains("Free space"),
            "{error}"
        );
        assert_eq!(fs::read_to_string(&target).unwrap(), "original");
    }

    #[test]
    fn changed_empty_file_id_requires_an_exact_staged_prefix() {
        let profile = tempfile::tempdir().unwrap();
        let target = profile.path().join("target");
        let root = profile.path().join("Recovery");
        let new = "complete revision after the first write";
        volume::FAT_PROBE.set(true);
        for valid in [true, false] {
            fs::write(&target, "").unwrap();
            use_faults(
                &[(Phase::Backup, unsupported), (Phase::Write, io_failure)],
                || save(&target, profile.path(), new).unwrap_err(),
            );
            let record = root
                .read_dir()
                .unwrap()
                .flatten()
                .map(|e| e.path())
                .find(|path| path.extension().is_some_and(|e| e == "save-journal"))
                .unwrap();
            let (_, mut journal) = read_journal(&root, &record).unwrap();
            assert!(journal.synthetic_empty_id);
            journal.target_entry.inode ^= 1;
            let moved = root.join(journal.target_entry.record_name());
            fs::rename(&record, &moved).unwrap();
            fs::write(&moved, serde_json::to_vec(&journal).unwrap()).unwrap();
            if !valid {
                fs::write(&target, format!("{}\0", &new[..8])).unwrap();
            }
            let result = recover_pending_save(&target, Some(profile.path()));
            if valid {
                assert!(result.unwrap().unwrap().content.is_some());
                assert_eq!(fs::read_to_string(&target).unwrap(), new);
            } else {
                let recovery = result.unwrap().unwrap();
                assert!(recovery.content.is_none());
                assert!(recovery.notice.contains("Staged revision kept at"));
                let archive = recovery
                    .notice
                    .split_once("Staged revision kept at ")
                    .unwrap()
                    .1
                    .split_once("; original kept at ")
                    .unwrap()
                    .0;
                assert_eq!(fs::read_to_string(archive).unwrap(), new);
                assert_eq!(
                    fs::read_to_string(&target).unwrap(),
                    format!("{}\0", &new[..8])
                );
                assert!(!moved.exists() && !journal.stage.get().exists());
            }
        }
        volume::FAT_PROBE.set(false);
    }

    #[test]
    fn remounted_forward_save_verifies_target_and_stage_before_replaying() {
        let profile = tempfile::tempdir().unwrap();
        let target = profile.path().join("target");
        let uuid = [6; 16];
        let original = "x".repeat(430_000);
        let new = "y".repeat(original.len());
        for check in ["partial", "unrelated target", "changed stage"] {
            fs::write(&target, &original).unwrap();
            volume::UUID_PROBE.set(Some(uuid));
            assert!(
                use_faults(
                    &[(Phase::Backup, unsupported), (Phase::Write, io_failure)],
                    || save(&target, profile.path(), &new)
                )
                .is_err()
            );
            let root = profile.path().join("Recovery");
            let entry = root
                .read_dir()
                .unwrap()
                .flatten()
                .map(|e| e.path())
                .find(|p| p.extension().is_some_and(|ext| ext == "save-journal"))
                .unwrap();
            let (_, mut journal) = read_journal(&root, &entry).unwrap();
            assert_eq!(journal.target_entry.volume, Some(uuid));
            journal.target_entry.device ^= 0x80;
            journal.stage_entry.device ^= 0x80;
            fs::write(&entry, serde_json::to_vec(&journal).unwrap()).unwrap();
            if check == "unrelated target" {
                fs::write(&target, "z".repeat(original.len())).unwrap(); // same inode, still unrelated
            } else if check == "changed stage" {
                fs::write(journal.stage.get(), &original).unwrap();
            }
            let notices = recover_saves(Some(profile.path()));
            if check == "partial" {
                assert!(
                    notices[0].starts_with("Recovered interrupted save:"),
                    "{notices:?}"
                );
                assert_eq!(fs::read_to_string(&target).unwrap(), new);
            } else if check == "unrelated target" {
                assert!(!entry.exists());
                assert!(
                    notices[0].contains("Staged revision kept at"),
                    "{notices:?}"
                );
                let archived = notices[0].split_once("Staged revision kept at ").unwrap().1;
                assert_eq!(fs::read_to_string(archived).unwrap(), new);
                assert_eq!(
                    fs::read_to_string(&target).unwrap(),
                    "z".repeat(original.len())
                );
                assert!(!journal.stage.get().exists());
                assert!(recover_saves(Some(profile.path())).is_empty());
            } else {
                assert!(entry.exists());
                assert!(
                    notices[0].contains("staged checksum changed"),
                    "{notices:?}"
                );
                assert_ne!(fs::read_to_string(&target).unwrap(), new);
                discard(&entry, &journal, false).unwrap();
            }
            volume::UUID_PROBE.set(None);
        }
    }

    #[test]
    fn frees_space_and_retries_a_pending_clone_without_relaunch() {
        let profile = tempfile::tempdir().unwrap();
        let target = profile.path().join("target");
        let original = "original bytes that should survive no-space and retry".repeat(8192);
        fs::write(&target, &original).unwrap();
        // A failing first pwrite need not copy any original extents back.
        let error = use_faults(
            &[
                (Phase::BeforeWrite, disk_full),
                (Phase::Rollback, || panic!("did not write any bytes")),
            ],
            || save(&target, profile.path(), "new").unwrap_err(),
        );
        assert!(error.contains("restored the original"), "{error}");
        assert_eq!(fs::read_to_string(&target).unwrap(), original);
        // A real partial write may still need more space to finish rollback.
        let error = use_faults(
            &[(Phase::Write, disk_full), (Phase::Rollback, disk_full)],
            || save(&target, profile.path(), "partial edit").unwrap_err(),
        );
        assert!(error.contains("rollback:"), "{error}");
        let saved = save(&target, profile.path(), "successful retry").unwrap();
        assert!(saved.warning.unwrap().contains("Restored original"));
        assert_eq!(fs::read_to_string(&target).unwrap(), "successful retry");
        assert!(recover_saves(Some(profile.path())).is_empty());
    }

    #[test]
    fn replacing_target_archives_the_undo_clone() {
        let profile = tempfile::tempdir().unwrap();
        let target = profile.path().join("target");
        fs::write(&target, "original".repeat(40_000)).unwrap();
        use_faults(
            &[(Phase::Write, io_failure), (Phase::Rollback, disk_full)],
            || save(&target, profile.path(), "partial edit").unwrap_err(),
        );
        let replacement = profile.path().join("replacement");
        fs::write(&replacement, "new inode").unwrap();
        fs::rename(replacement, &target).unwrap();
        let notices = recover_saves(Some(profile.path()));
        assert_eq!(notices.len(), 1, "{notices:?}");
        let (_, archive) = notices[0].split_once("Rollback copy kept at ").unwrap();
        assert_eq!(
            fs::read_to_string(archive).unwrap(),
            "original".repeat(40_000)
        );
        assert!(recover_saves(Some(profile.path())).is_empty());
        assert_eq!(fs::read_to_string(&target).unwrap(), "new inode");
    }

    #[test]
    #[allow(unsafe_code)]
    fn killed_mid_pwrite_is_replayed_only_for_the_original_inode() {
        const CHILD: &str = "VELOCITY_TEST_SAVE_CHILD";
        const MODE: &str = "VELOCITY_TEST_SAVE_MODE";
        const NEW: &str = "complete saved revision for both links";
        const OLD: &str = "original revision before the child writes any bytes";
        if let Some(profile) = std::env::var_os(CHILD) {
            let profile = Path::new(&profile);
            let mode = std::env::var(MODE).unwrap();
            let mut faults: Vec<Fault> = vec![(
                if mode == "complete" {
                    Phase::AfterSync
                } else {
                    Phase::Write
                },
                || {
                    // SAFETY: Kill only this dedicated subprocess, after the first pwrite.
                    unsafe { libc::raise(libc::SIGKILL) };
                    unreachable!()
                },
            )];
            if mode == "forward" {
                faults.push((Phase::Backup, unsupported));
            }
            use_faults(&faults, || save(&profile.join("target"), profile, NEW)).unwrap();
            panic!("save did not stop");
        }
        for (mode, replaced, cross_volume) in [
            ("forward", false, false),
            ("forward", true, false),
            ("forward", true, true),
            ("rollback", false, false),
            ("complete", false, false),
        ] {
            let profile = tempfile::tempdir().unwrap();
            let target = profile.path().join("target");
            let alias = profile.path().join("alias");
            fs::write(&target, OLD).unwrap();
            fs::hard_link(&target, &alias).unwrap();
            let ino = target.metadata().unwrap().ino();
            let status = Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "save_journal::tests::killed_mid_pwrite_is_replayed_only_for_the_original_inode"])
                .env(CHILD, profile.path())
                .env(MODE, mode)
                .status()
                .unwrap();
            assert_eq!(status.signal(), Some(libc::SIGKILL));
            assert_eq!(
                fs::read_to_string(&target).unwrap() == NEW,
                mode == "complete"
            );
            if replaced {
                fs::remove_file(&target).unwrap();
                fs::write(&target, "replacement from another app").unwrap();
                fs::set_permissions(&target, fs::Permissions::from_mode(0o400)).unwrap();
            }
            let recovery = profile.path().join("Recovery");
            let messages = if cross_volume {
                let stage_path = fs::read_dir(&recovery)
                    .unwrap()
                    .flatten()
                    .map(|entry| entry.path())
                    .find(|path| staged_name(path, STAGE))
                    .unwrap();
                let companion = stage_path.with_file_name(format!(
                    "._{}",
                    stage_path.file_name().unwrap().to_string_lossy()
                ));
                fs::write(&companion, "AppleDouble fixture").unwrap();
                let first = use_faults(
                    &[
                        (Phase::ArchiveRename, || {
                            Err(io::Error::from_raw_os_error(libc::EXDEV))
                        }),
                        (Phase::ArchivePublished, io_failure),
                    ],
                    || recover_saves(Some(profile.path())),
                );
                assert!(first[0].contains("cleanup failed"), "{first:?}");
                // A relaunch after publication reuses the verified copy.
                let messages = recover_saves(Some(profile.path()));
                assert!(!companion.exists());
                messages
            } else {
                recover_saves(Some(profile.path()))
            };
            assert_eq!(messages.len(), 1, "{messages:?}");
            if replaced {
                assert!(
                    messages[0].contains("file identity changed"),
                    "{messages:?}"
                );
                let entries: Vec<_> = fs::read_dir(&recovery)
                    .unwrap()
                    .flatten()
                    .map(|e| e.path())
                    .collect();
                assert_eq!(entries.len(), 2, "{entries:?}");
                let mut versions: Vec<_> = entries
                    .iter()
                    .map(|archive| {
                        assert!(
                            archive
                                .file_name()
                                .unwrap()
                                .to_str()
                                .unwrap()
                                .ends_with("-target")
                        );
                        assert!(messages[0].contains(archive.to_str().unwrap()));
                        fs::read_to_string(archive).unwrap()
                    })
                    .collect();
                versions.sort();
                assert_eq!(versions, [NEW, OLD]);
                assert!(
                    fs::read_dir(profile.path())
                        .unwrap()
                        .flatten()
                        .all(|entry| {
                            !entry
                                .file_name()
                                .as_encoded_bytes()
                                .starts_with(STAGE.as_bytes())
                        })
                );
                assert!(recover_saves(Some(profile.path())).is_empty());
                assert_eq!(
                    fs::read_to_string(&target).unwrap(),
                    "replacement from another app"
                );
            } else {
                let action = match mode {
                    "forward" => "Recovered interrupted save:",
                    "rollback" => "Restored original after interrupted save:",
                    _ => "Completed save already intact:",
                };
                assert!(messages[0].starts_with(action), "{messages:?}");
                assert_eq!(target.metadata().unwrap().ino(), ino);
                let expected = if mode == "rollback" { OLD } else { NEW };
                assert_eq!(fs::read_to_string(&target).unwrap(), expected);
                assert_eq!(fs::read_to_string(&alias).unwrap(), expected);
                assert!(recover_saves(Some(profile.path())).is_empty());
            }
        }
    }
}
