#[cfg(not(unix))]
use crate::atomic::Stage;
#[cfg(unix)]
use crate::save_journal;
#[path = "save_stream.rs"]
mod stream;
use crate::{
    ContentIdentity, Format,
    atomic::{self, Durability},
    encoding, identity, regular, write_encoded,
};
use std::{
    fs::{self, File, Metadata},
    io::{self, BufWriter, Write},
    path::Path,
};
use vel_text::Text;

/// The result of publishing a complete new document revision.
#[derive(Debug)]
pub struct Saved {
    /// Raw bytes' identity, computed during encoding.
    pub content: ContentIdentity,
    /// Nonfatal durability diagnostic that should be shown once to the user.
    pub warning: Option<String>,
}

pub(crate) const INLINE_LIMIT: usize = 256 * 1024;

#[derive(Clone, Copy)]
pub(super) struct Encoded<'a> {
    pub bytes: &'a [u8],
    pub content: ContentIdentity,
}

/// Saves a document using staged replacement or a journaled inode-preserving write.
pub fn save(path: &Path, text: &Text, format: &Format, atomic: bool) -> Result<(), String> {
    save_with_identity(path, text, format, atomic).map(|_| ())
}

/// Saves and returns the raw identity without rereading the destination.
pub fn save_with_identity(
    path: &Path,
    text: &Text,
    format: &Format,
    atomic: bool,
) -> Result<ContentIdentity, String> {
    save_report(path, text, format, atomic, None).map(|saved| {
        if let Some(warning) = saved.warning {
            eprintln!("{warning}");
        }
        saved.content
    })
}

/// Saves with an optional data profile for journal recovery, and reports durability warnings.
pub fn save_report(
    path: &Path,
    text: &Text,
    format: &Format,
    atomic: bool,
    profile: Option<&Path>,
) -> Result<Saved, String> {
    save_report_with_expected(path, text, format, atomic, profile, None)
}

/// As [`save_report`], with the last accepted raw disk identity. Saves that
/// waited for a writer, or are on volumes with coarse timestamps, check it
/// again under the save lock before changing the document.
pub fn save_report_with_expected(
    path: &Path,
    text: &Text,
    format: &Format,
    atomic: bool,
    profile: Option<&Path>,
    expected_disk: Option<ContentIdentity>,
) -> Result<Saved, String> {
    let codec = encoding(&format.encoding)?;
    if text.len_bytes() <= INLINE_LIMIT {
        let mut digest = identity::Digest::default();
        let mut bytes = Vec::with_capacity(text.len_bytes());
        write_encoded(
            &mut identity::DigestWriter {
                output: &mut bytes,
                digest: &mut digest,
            },
            text,
            format,
            codec,
        )?;
        let content = digest.identity();
        return replace_inner(
            path,
            atomic,
            profile,
            Some(content.bytes),
            Some(Encoded {
                bytes: &bytes,
                content,
            }),
            expected_disk,
            |output| {
                output.write_all(&bytes).map_err(|e| e.to_string())?;
                Ok(content)
            },
        );
    }
    let exact_len = (codec == encoding_rs::UTF_8 && format.line_ending == crate::LineEnding::Lf)
        .then(|| text.len_bytes() as u64 + if format.bom { 3 } else { 0 });
    replace_inner(
        path,
        atomic,
        profile,
        exact_len,
        None,
        expected_disk,
        |output| {
            if text.len_bytes() >= 128 * 1024 * 1024 {
                return stream::encode(output, text, format, codec);
            }
            let mut digest = identity::Digest::default();
            // Hash each coalesced write instead of repeatedly feeding tiny rope
            // leaves to the hasher. There is no extra pass over the document.
            let mut writer = BufWriter::with_capacity(
                4 * 1024 * 1024,
                identity::DigestWriter {
                    output,
                    digest: &mut digest,
                },
            );
            write_encoded(&mut writer, text, format, codec)?;
            drop(writer);
            Ok(digest.identity())
        },
    )
}

#[cfg(all(test, target_os = "macos"))]
fn replace(
    path: &Path,
    prefer_rename: bool,
    profile: Option<&Path>,
    encode: impl Fn(&mut (dyn Write + Send)) -> Result<ContentIdentity, String>,
) -> Result<Saved, String> {
    replace_inner(path, prefer_rename, profile, None, None, None, encode)
}

fn replace_inner(
    path: &Path,
    prefer_rename: bool,
    profile: Option<&Path>,
    exact_len: Option<u64>,
    encoded: Option<Encoded<'_>>,
    expected_disk: Option<ContentIdentity>,
    encode: impl Fn(&mut (dyn Write + Send)) -> Result<ContentIdentity, String>,
) -> Result<Saved, String> {
    let target = match fs::symlink_metadata(path) {
        Ok(_) => fs::canonicalize(path).map_err(|e| regular::error(path, e))?,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            let name = path
                .file_name()
                .ok_or_else(|| regular::error(path, "no file name"))?;
            fs::canonicalize(atomic::parent(path))
                .map_err(|e| regular::error(path, e))?
                .join(name)
        }
        Err(e) => return Err(regular::error(path, e)),
    };
    #[cfg(unix)]
    let cleanup = save_journal::clean_stages(profile, atomic::parent(&target));
    let result = replace_target(
        &target,
        prefer_rename,
        profile,
        exact_len,
        encoded,
        expected_disk,
        encode,
    );
    #[cfg(unix)]
    if !cleanup.is_empty() {
        let notes = cleanup.join("; ");
        return result
            .map(|mut saved| {
                saved.warning = Some(
                    saved
                        .warning
                        .map_or_else(|| notes.clone(), |w| format!("{w}; {notes}")),
                );
                saved
            })
            .map_err(|error| format!("{error}; {notes}"));
    }
    result
}

fn replace_target(
    target: &Path,
    prefer_rename: bool,
    profile: Option<&Path>,
    exact_len: Option<u64>,
    encoded: Option<Encoded<'_>>,
    expected_disk: Option<ContentIdentity>,
    encode: impl Fn(&mut (dyn Write + Send)) -> Result<ContentIdentity, String>,
) -> Result<Saved, String> {
    #[cfg(unix)]
    let new_stage = |mode| save_journal::tracked_stage(profile, atomic::parent(target), mode);
    #[cfg(not(unix))]
    let new_stage = |mode| Stage::new(atomic::parent(target), ".velocity-save-", mode);
    let error = |e: io::Error| regular::error(target, e);
    let existing = regular::metadata(target, true)?;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    #[cfg(windows)]
    let _ = (prefer_rename, profile);
    #[cfg(not(any(unix, windows)))]
    let _ = (prefer_rename, profile, expected_disk);
    #[cfg(not(target_os = "macos"))]
    let _ = (exact_len, encoded.map(|image| (image.bytes, image.content)));
    let Some(mut expected) = existing else {
        #[cfg(windows)]
        if expected_disk.is_some() {
            return Err(changed(target));
        }
        #[cfg(unix)]
        let mode = Some(fs::Permissions::from_mode(0o666));
        #[cfg(not(unix))]
        let mode = None;
        let mut stage = new_stage(mode).map_err(error)?;
        let content = encode(stage.file()).map_err(|e| regular::error(target, e))?;
        stage.sync(Durability::Full).map_err(error)?;
        return stage
            .publish(target, true)
            .map(|p| Saved {
                content,
                warning: p.warning,
            })
            .map_err(error);
    };

    // Renaming must not bypass a read-only or locked file.
    #[cfg(target_os = "macos")]
    {
        use std::os::macos::fs::MetadataExt;
        if expected.st_flags()
            & (libc::UF_IMMUTABLE | libc::SF_IMMUTABLE | libc::UF_APPEND | libc::SF_APPEND)
            != 0
        {
            return Err(regular::error(target, "file is locked or append-only"));
        }
    }
    #[cfg(windows)]
    if expected.permissions().readonly() {
        return Err(regular::error(target, "file is read-only"));
    }
    let writable = regular::writable(target)?;
    let opened = writable.metadata().map_err(error)?;
    if regular::decompressed_on_open(&expected, &opened) {
        expected = opened;
    }
    #[cfg(unix)]
    let record = save_journal::begin(target, &writable, &mut expected, profile, expected_disk)?;
    #[cfg(unix)]
    let with_notice = |mut saved: Saved| {
        if let Some(recovered) = &record.recovery_notice {
            saved.warning = Some(saved.warning.map_or_else(
                || recovered.clone(),
                |warning| format!("{recovered}; {warning}"),
            ));
        }
        saved
    };
    #[cfg(not(unix))]
    let with_notice = |saved: Saved| saved;
    if !same_version(&expected, &writable.metadata().map_err(error)?) {
        return Err(changed(target));
    }
    let source = regular::read(target)
        .ok()
        .filter(|source| source.metadata().is_ok_and(|m| same_version(&expected, &m)));
    // ST3 defaults to in-place and adaptively disables atomic_save on Unix
    // when permissions cannot be preserved: https://www.sublimetext.com/3
    #[cfg(unix)]
    let mut rename = prefer_rename && !has_links(&expected) && owns_file(&expected);
    #[cfg(not(unix))]
    let rename = true;
    #[cfg(target_os = "macos")]
    if !rename
        && let (Some(source), Some(len)) = (source.as_ref(), exact_len)
        && let Some(undo) = record.prepare_undo(source, &expected, len, encoded)?
    {
        return undo
            .write(&writable, &expected, len, encode)
            .map(with_notice);
    }
    #[cfg(unix)]
    let stage = if rename {
        record.sibling_stage()
    } else {
        // Keeping the journal and its data on the profile volume requires
        // only one recovery-device flush on a cross-volume in-place save.
        record.stage()
    };
    #[cfg(not(unix))]
    let stage = new_stage(None);
    let mut stage = match stage {
        Ok(stage) => stage,
        #[cfg(unix)]
        Err(e) if adaptively_in_place(&e) => {
            rename = false;
            record
                .stage()
                .map_err(|e| save_journal::stage_error(target, e))?
        }
        #[cfg(unix)]
        Err(e) => return Err(save_journal::stage_error(target, e)),
        #[cfg(not(unix))]
        Err(e) => return Err(error(e)),
    };
    let content = encode(stage.file()).map_err(|e| {
        let detail = regular::error(target, e);
        #[cfg(unix)]
        if !rename && save_journal::out_of_space(&detail) {
            return format!("{detail}; this volume needs space for a complete staged revision. Free space and retry");
        }
        detail
    })?;
    let metadata = if rename {
        source
            .as_ref()
            .ok_or_else(|| io::Error::other("cannot read the original metadata"))
            .and_then(|source| copy_metadata(source, stage.file(), &expected))
    } else {
        Ok(())
    };
    #[cfg(unix)]
    match metadata {
        Err(e) if adaptively_in_place(&e) || source.is_none() => rename = false,
        result => result.map_err(error)?,
    }
    #[cfg(not(unix))]
    metadata.map_err(error)?;
    #[cfg(windows)]
    super::windows::copy_dacl(source.as_ref().expect("metadata source"), stage.path())
        .map_err(error)?;
    // POSIX has no compare-and-replace rename; an external writer can still race commit.
    check_target(target, &expected)?;
    if rename {
        stage.sync(Durability::Full).map_err(error)?;
        #[cfg(windows)]
        if let Some(disk) = expected_disk
            && crate::content_identity(target)? != Some(disk)
        {
            return Err(changed(target));
        }
        check_target(target, &expected)?;
        match stage.replace_existing(target) {
            Ok(published) => {
                return Ok(with_notice(Saved {
                    content,
                    warning: published.warning,
                }));
            }
            #[cfg(unix)]
            Err(e) if adaptively_in_place(&e) => check_target(target, &expected)?,
            Err(e) => return Err(error(e)),
        }
    }
    #[cfg(unix)]
    {
        let warning = record.write(stage, &writable, source.as_ref(), &expected, content)?;
        Ok(with_notice(Saved { content, warning }))
    }
    #[cfg(not(unix))]
    unreachable!("non-Unix saves use replacement")
}

pub(super) fn check_target(target: &Path, expected: &Metadata) -> Result<(), String> {
    let current = fs::symlink_metadata(target).map_err(|e| regular::error(target, e))?;
    if !current.is_file() || !same_version(expected, &current) {
        return Err(changed(target));
    }
    Ok(())
}

pub(super) fn changed(path: &Path) -> String {
    regular::error(path, "file changed while staging save; review it and retry")
}

#[cfg(unix)]
fn has_links(metadata: &Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    metadata.nlink() > 1
}

#[cfg(unix)]
fn owns_file(metadata: &Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    #[cfg(test)]
    if FOREIGN_OWNER.get() {
        return false;
    }
    metadata.uid() == rustix::process::geteuid().as_raw()
}

#[cfg(all(test, unix))]
thread_local! { static FOREIGN_OWNER: std::cell::Cell<bool> = const { std::cell::Cell::new(false) }; }

pub(super) fn same_version(a: &Metadata, b: &Metadata) -> bool {
    #[cfg(all(test, unix))]
    if save_journal::COARSE_VERSION_PROBE.get() {
        // ctime cannot be set by a user process. Model a volume whose ctime
        // tick has not advanced; the test restores mtime on the real file.
        return crate::same_entry(a, b)
            && a.len() == b.len()
            && a.modified().ok() == b.modified().ok();
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if !crate::same_entry(a, b)
            || (a.ctime(), a.ctime_nsec(), a.nlink()) != (b.ctime(), b.ctime_nsec(), b.nlink())
        {
            return false;
        }
    }
    a.len() == b.len() && a.modified().ok() == b.modified().ok()
}

#[cfg(unix)]
fn adaptively_in_place(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::PermissionDenied | io::ErrorKind::ReadOnlyFilesystem
    ) || atomic::unsupported(e)
        || [
            rustix::io::Errno::BUSY,
            rustix::io::Errno::XDEV,
            rustix::io::Errno::TXTBSY,
        ]
        .iter()
        .any(|code| Some(code.raw_os_error()) == e.raw_os_error())
}

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
fn fcopy(source: &File, staged: &File, flags: libc::copyfile_flags_t) -> io::Result<()> {
    use std::os::fd::AsRawFd;
    // https://github.com/apple-oss-distributions/copyfile/blob/main/copyfile.3
    // SAFETY: descriptors refer to checked regular files and null requests default state.
    if unsafe {
        libc::fcopyfile(
            source.as_raw_fd(),
            staged.as_raw_fd(),
            std::ptr::null_mut(),
            flags,
        )
    } == 0
    {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(target_os = "macos")]
fn copy_metadata(source: &File, staged: &File, metadata: &Metadata) -> io::Result<()> {
    use rustix::{
        fs::{XattrFlags, fgetxattr, fgetxattr_query, fsetxattr},
        io::Errno,
    };
    use std::os::macos::fs::MetadataExt;
    const QUARANTINE: &str = "com.apple.quarantine";
    let quarantine = match fgetxattr_query(source, QUARANTINE) {
        Ok(len) => {
            let mut value = vec![0; len];
            let count = fgetxattr(source, QUARANTINE, &mut value[..])?;
            value.truncate(count);
            Some(value)
        }
        Err(Errno::NOATTR) => None,
        Err(e) if atomic::unsupported(&e.into()) => None,
        Err(e) => return Err(e.into()),
    };
    fcopy(source, staged, libc::COPYFILE_METADATA)?;
    if let Some(value) = quarantine {
        // copyfile applies quarantine policy; a document save must keep the tag verbatim.
        fsetxattr(staged, QUARANTINE, &value, XattrFlags::empty())?;
    }
    // copyfile only restores user-settable flags, not document ID/compression
    // bookkeeping or privileged flags:
    // https://github.com/apple-oss-distributions/copyfile/blob/main/copyfile.c
    let flags = |m: &Metadata| m.st_flags() & 0xffff & !(libc::UF_TRACKED | libc::UF_COMPRESSED);
    let copied = staged.metadata()?;
    if (
        metadata.st_uid(),
        metadata.st_gid(),
        metadata.st_mode(),
        flags(metadata),
    ) != (
        copied.st_uid(),
        copied.st_gid(),
        copied.st_mode(),
        flags(&copied),
    ) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "could not preserve file ownership, mode or flags",
        ));
    }
    copy_creation_date(staged, metadata, &copied)?;
    staged.set_modified(std::time::SystemTime::now())
}

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
fn copy_creation_date(staged: &File, source: &Metadata, copy: &Metadata) -> io::Result<()> {
    use std::{
        mem,
        os::{fd::AsRawFd, macos::fs::MetadataExt},
    };
    if source.created().ok() == copy.created().ok() {
        return Ok(());
    }
    // SAFETY: all fields are integer flags.
    let mut attrs: libc::attrlist = unsafe { mem::zeroed() };
    attrs.bitmapcount = libc::ATTR_BIT_MAP_COUNT;
    attrs.commonattr = libc::ATTR_CMN_CRTIME;
    let date = libc::timespec {
        tv_sec: source.st_birthtime(),
        tv_nsec: source.st_birthtime_nsec(),
    };
    // SAFETY: attrlist requests only creation time; buffer contains one timespec.
    let status = unsafe {
        libc::fsetattrlist(
            staged.as_raw_fd(),
            &raw mut attrs as *mut _,
            (&raw const date).cast_mut().cast(),
            mem::size_of_val(&date),
            0,
        )
    };
    if status == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
fn copy_metadata(source: &File, staged: &File, metadata: &Metadata) -> io::Result<()> {
    use rustix::fs::{Gid, Uid, XattrFlags, fchown, fgetxattr, flistxattr, fsetxattr};
    use std::os::unix::fs::MetadataExt;
    let ours = staged.metadata()?;
    if (ours.uid(), ours.gid()) != (metadata.uid(), metadata.gid()) {
        fchown(
            staged,
            Some(Uid::from_raw(metadata.uid())),
            Some(Gid::from_raw(metadata.gid())),
        )?;
    }
    staged.set_permissions(metadata.permissions())?;
    let mut names = vec![0; rustix::fs::flistxattr_query(source)?];
    let len = flistxattr(source, names.as_mut_slice())?;
    for name in names[..len].split(|&b| b == 0).filter(|n| !n.is_empty()) {
        let mut value = vec![0; rustix::fs::fgetxattr_query(source, name)?];
        let len = fgetxattr(source, name, value.as_mut_slice())?;
        fsetxattr(staged, name, &value[..len], XattrFlags::empty())?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn copy_metadata(_source: &File, staged: &File, metadata: &Metadata) -> io::Result<()> {
    staged.set_permissions(metadata.permissions())
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use crate::atomic::{FAULT, Point};
    use std::{
        os::unix::{
            fs::{MetadataExt, PermissionsExt, symlink},
            process::ExitStatusExt,
        },
        process::Command,
    };

    #[test]
    fn disk_full_quota_permission_and_publish_failure_keep_original() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("document");
        fs::write(&target, b"original").unwrap();
        let meta = target.metadata().unwrap();
        rustix::fs::setxattr(
            &target,
            "com.velocity.test",
            b"tag",
            rustix::fs::XattrFlags::empty(),
        )
        .unwrap();
        for code in [libc::ENOSPC, libc::EDQUOT, libc::EACCES, libc::EIO] {
            assert!(
                replace(&target, true, Some(dir.path()), |file| {
                    file.write_all(b"partial").unwrap();
                    Err(io::Error::from_raw_os_error(code).to_string())
                })
                .is_err()
            );
        }
        for (point, code) in [
            (Point::Sync, libc::EIO),
            (Point::Publish, libc::ENOSPC),
            (Point::Replace, libc::EIO),
        ] {
            FAULT.set(Some((point, code)));
            let result = save_report(
                &target,
                &Text::from("whole"),
                &Format::default(),
                true,
                Some(dir.path()),
            );
            FAULT.set(None);
            assert!(result.is_err(), "{result:?}");
        }
        assert_eq!(fs::read(&target).unwrap(), b"original");
        assert_eq!(target.metadata().unwrap().ino(), meta.ino());
        let mut value = [0; 3];
        rustix::fs::getxattr(&target, "com.velocity.test", &mut value[..]).unwrap();
        assert_eq!(&value, b"tag");
        assert_eq!(dir.path().join("Recovery").read_dir().unwrap().count(), 0);
        assert!(!dir.path().read_dir().unwrap().flatten().any(|entry| {
            entry
                .file_name()
                .as_encoded_bytes()
                .starts_with(b".velocity-save-")
        }));
        // The same guarantee covers a first save: failure never creates the final name.
        let new = dir.path().join("first-save");
        assert!(
            replace(&new, true, Some(dir.path()), |file| {
                file.write_all(b"partial").unwrap();
                Err("simulated ENOSPC".into())
            })
            .is_err()
        );
        assert!(!new.exists());
    }

    #[test]
    #[allow(unsafe_code)]
    fn killed_during_staged_write_keeps_original() {
        const CHILD: &str = "VELOCITY_SAVE_TEST_DESTINATION";
        if let Some(path) = std::env::var_os(CHILD) {
            let path = Path::new(&path);
            let _ = replace(path, true, path.parent(), |file| {
                file.write_all(b"partial").unwrap();
                // SAFETY: the child kills only itself, bypassing all destructors.
                unsafe { libc::raise(libc::SIGKILL) };
                unreachable!()
            });
            panic!("failed to enter staged write");
        }
        for relaunch in [true, false] {
            let dir = tempfile::tempdir().unwrap();
            let target = dir.path().join("document");
            fs::write(&target, b"original").unwrap();
            let status = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "save::tests::killed_during_staged_write_keeps_original",
                ])
                .env(CHILD, &target)
                .status()
                .unwrap();
            assert_eq!(status.signal(), Some(libc::SIGKILL));
            assert_eq!(fs::read(&target).unwrap(), b"original");
            let notice = if relaunch {
                crate::recover_saves(Some(dir.path())).join("; ")
            } else {
                save_report(
                    &target,
                    &Text::from("next revision"),
                    &Format::default(),
                    true,
                    Some(dir.path()),
                )
                .unwrap()
                .warning
                .unwrap()
            };
            assert!(notice.contains("Removed incomplete save stage"), "{notice}");
            assert_eq!(dir.path().join("Recovery").read_dir().unwrap().count(), 0);
            assert_eq!(dir.path().read_dir().unwrap().count(), 2);
        }
    }

    #[test]
    fn save_through_alias_keeps_group_mode_xattr_acl_and_creation_date() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("document");
        let alias = dir.path().join("alias");
        fs::write(&target, b"original").unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o640)).unwrap();
        let status = Command::new("/bin/chmod")
            .args(["+a", "everyone deny execute"])
            .arg(&target)
            .status()
            .unwrap();
        assert!(status.success());
        let acl = |path: &Path| {
            let output = Command::new("/bin/ls")
                .arg("-le")
                .arg(path)
                .output()
                .unwrap();
            String::from_utf8(output.stdout)
                .unwrap()
                .lines()
                .nth(1)
                .unwrap()
                .to_owned()
        };
        let before_acl = acl(&target);
        rustix::fs::setxattr(
            &target,
            "com.velocity.test",
            b"tag",
            rustix::fs::XattrFlags::empty(),
        )
        .unwrap();
        symlink(&target, &alias).unwrap();
        let before = target.metadata().unwrap();
        save_report(
            &alias,
            &Text::from("complete"),
            &Format::default(),
            true,
            Some(dir.path()),
        )
        .unwrap();
        let after = target.metadata().unwrap();
        assert_ne!(before.ino(), after.ino());
        assert_eq!(
            (before.mode(), before.uid(), before.gid()),
            (after.mode(), after.uid(), after.gid())
        );
        assert_eq!(before.created().unwrap(), after.created().unwrap());
        assert_eq!(before_acl, acl(&target));
        let mut value = [0; 3];
        rustix::fs::getxattr(&target, "com.velocity.test", &mut value[..]).unwrap();
        assert_eq!(&value, b"tag");
        assert!(alias.symlink_metadata().unwrap().is_symlink());
        assert_eq!(fs::read(alias).unwrap(), b"complete");
    }

    #[test]
    fn default_and_adaptive_saves_preserve_the_open_inode() {
        let dir = tempfile::tempdir().unwrap();
        let parent = dir.path().join("files");
        fs::create_dir(&parent).unwrap();
        let target = parent.join("document");
        let link = parent.join("hardlink");
        for (atomic, hardlinked, unwritable, foreign, bind_mount) in [
            (false, false, false, false, false),
            (false, true, false, false, false),
            (false, false, true, false, false),
            (true, true, false, false, false),
            (true, false, true, false, false),
            (true, false, false, true, false),
            (true, false, false, false, true),
        ] {
            fs::write(&target, b"original").unwrap();
            fs::set_permissions(&target, fs::Permissions::from_mode(0o660)).unwrap();
            let held = File::open(&target).unwrap();
            let inode = held.metadata().unwrap().ino();
            if hardlinked {
                fs::hard_link(&target, &link).unwrap();
            }
            if unwritable {
                fs::set_permissions(&parent, fs::Permissions::from_mode(0o555)).unwrap();
            }
            FOREIGN_OWNER.set(foreign);
            if bind_mount {
                FAULT.set(Some((Point::Replace, libc::EBUSY)));
            }
            let result = save_report(
                &target,
                &Text::from("complete longer revision"),
                &Format::default(),
                atomic,
                Some(dir.path()),
            );
            FAULT.set(None);
            FOREIGN_OWNER.set(false);
            fs::set_permissions(&parent, fs::Permissions::from_mode(0o700)).unwrap();
            assert!(result.unwrap().warning.is_none());
            assert_eq!(target.metadata().unwrap().ino(), inode);
            use std::io::Read;
            let mut bytes = String::new();
            (&held).read_to_string(&mut bytes).unwrap();
            assert_eq!(bytes, "complete longer revision");
            assert_eq!(fs::read(&target).unwrap(), bytes.as_bytes());
            assert_eq!(target.metadata().unwrap().mode() & 0o777, 0o660);
            if hardlinked {
                assert_eq!(fs::read(&link).unwrap(), bytes.as_bytes());
                fs::remove_file(&link).unwrap();
            }
        }
    }

    #[test]
    fn directory_sync_failure_warns_after_publishing_complete_file() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("document");
        fs::write(&target, b"original").unwrap();
        atomic::DIRECTORY_DESTINATION.with(|p| p.replace(Some(target.canonicalize().unwrap())));
        FAULT.set(Some((Point::Directory, libc::EACCES)));
        let result = save_report(
            &target,
            &Text::from("complete"),
            &Format::default(),
            true,
            Some(dir.path()),
        );
        FAULT.set(None);
        atomic::DIRECTORY_DESTINATION.with(|p| p.replace(None));
        assert!(
            result
                .unwrap()
                .warning
                .unwrap()
                .contains("could not be synced")
        );
        assert_eq!(fs::read(target).unwrap(), b"complete");
    }

    #[test]
    fn first_save_claims_the_name_without_exclusive_rename_or_hard_links() {
        let root = tempfile::tempdir().unwrap();
        atomic::EXFAT_PROBE.set(true);
        for atomic in [false, true] {
            let path = root.path().join(format!("new-{atomic}.txt"));
            save_report(
                &path,
                &Text::from("complete first save"),
                &Format::default(),
                atomic,
                Some(root.path()),
            )
            .unwrap();
            assert_eq!(fs::read(&path).unwrap(), b"complete first save");
        }
        crate::atomic::EXFAT_PROBE.set(false);
        assert_eq!(root.path().join("Recovery").read_dir().unwrap().count(), 0);
    }

    #[test]
    fn atomic_save_adapts_when_replacement_is_not_supported() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("fixture.txt");
        fs::write(&path, b"original").unwrap();
        let inode = path.metadata().unwrap().ino();
        FAULT.set(Some((Point::Replace, libc::ENOTSUP)));
        let result = save_report(
            &path,
            &Text::from("revision"),
            &Format::default(),
            true,
            Some(root.path()),
        );
        FAULT.set(None);
        assert!(result.is_ok(), "{result:?}");
        assert_eq!(path.metadata().unwrap().ino(), inode);
        assert_eq!(fs::read(&path).unwrap(), b"revision");
    }

    #[test]
    fn successful_local_saves_use_one_full_media_flush() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("local.txt");
        let save = |bytes, atomic| {
            save_report(
                &path,
                &Text::from("a".repeat(bytes).as_str()),
                &Format::default(),
                atomic,
                Some(root.path()),
            )
            .unwrap()
        };
        save(1024, false);
        for (bytes, atomic, barriers, directory_syncs) in [
            (1024, false, 1, 1),
            (128 * 1024, false, 1, 1),
            (512 * 1024, false, 1, 1),
            (1024, true, 1, 2),
        ] {
            atomic::FLUSHES.with(|f| f.replace(Some(Vec::new())));
            save(bytes, atomic);
            let trace = atomic::FLUSHES.with(|f| f.take().unwrap());
            assert_eq!(
                trace
                    .iter()
                    .filter(|d| **d == Some(Durability::Full))
                    .count(),
                1,
                "{trace:?}"
            );
            assert_eq!(
                trace
                    .iter()
                    .filter(|d| **d == Some(Durability::Barrier))
                    .count(),
                barriers,
                "{trace:?}"
            );
            assert_eq!(
                trace.iter().filter(|d| d.is_none()).count(),
                directory_syncs,
                "{trace:?}"
            );
        }
    }

    #[test]
    #[allow(unsafe_code)]
    fn tracked_files_save_and_quarantine_is_byte_exact_under_both_settings() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("tracked.txt");
        let quarantine = b"0081;65aaf49a;Safari;42DE2390-608A-444F-8119-91FE3013CFA6";
        for atomic in [false, true] {
            fs::write(&target, b"original").unwrap();
            let descriptor = fs::File::open(&target).unwrap();
            use std::os::fd::AsRawFd;
            // SAFETY: a live fixture descriptor and an ordinary Darwin user flag.
            let status = unsafe { libc::fchflags(descriptor.as_raw_fd(), libc::UF_TRACKED) };
            assert_eq!(status, 0);
            rustix::fs::setxattr(
                &target,
                "com.apple.quarantine",
                quarantine,
                rustix::fs::XattrFlags::empty(),
            )
            .unwrap();
            let inode = descriptor.metadata().unwrap().ino();
            save_report(
                &target,
                &Text::from("complete revision"),
                &Format::default(),
                atomic,
                Some(dir.path()),
            )
            .unwrap();
            assert_eq!(fs::read(&target).unwrap(), b"complete revision");
            assert_eq!(target.metadata().unwrap().ino() != inode, atomic);
            let mut value = [0; 1024];
            let len =
                rustix::fs::getxattr(&target, "com.apple.quarantine", &mut value[..]).unwrap();
            assert_eq!(&value[..len], quarantine);
        }
    }

    #[test]
    #[ignore = "requires a volume where ditto can create a genuinely compressed file"]
    fn first_write_to_a_native_compressed_file_keeps_the_document() {
        use std::os::macos::fs::MetadataExt;
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("plain");
        let path = root.path().join("compressed.txt");
        let original = "compressible ASCII fixture\n".repeat(1024);
        fs::write(&source, &original).unwrap();
        assert!(
            Command::new("/usr/bin/ditto")
                .arg("--hfsCompression")
                .args([&source, &path])
                .status()
                .unwrap()
                .success()
        );
        assert_ne!(path.metadata().unwrap().st_flags() & libc::UF_COMPRESSED, 0);
        let inode = path.metadata().unwrap().ino();
        let new = format!("new revision\n{original}");
        save_report(
            &path,
            &Text::from(new.as_str()),
            &Format::default(),
            false,
            Some(root.path()),
        )
        .unwrap();
        assert_eq!(path.metadata().unwrap().ino(), inode);
        assert_eq!(fs::read_to_string(&path).unwrap(), new);
    }
}
