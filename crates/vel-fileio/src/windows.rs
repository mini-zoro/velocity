//! Same-volume publication and shell operations; no remove-then-rename fallback.
use std::{
    ffi::OsString,
    fs::File,
    io,
    os::windows::{ffi::OsStrExt, io::AsRawHandle},
    path::{Path, Prefix},
    ptr::null_mut,
};
use windows_sys::Win32::{
    Foundation::{ERROR_SUCCESS, LocalFree},
    Security::{
        ACL,
        Authorization::{GetSecurityInfo, SE_FILE_OBJECT, SetNamedSecurityInfoW},
        DACL_SECURITY_INFORMATION, GetSecurityDescriptorControl,
        PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, SE_DACL_PROTECTED,
        UNPROTECTED_DACL_SECURITY_INFORMATION,
    },
    Storage::FileSystem::{
        FILE_ATTRIBUTE_NORMAL, GetVolumeInformationByHandleW, MOVEFILE_REPLACE_EXISTING,
        MOVEFILE_WRITE_THROUGH, MoveFileExW, SetFileAttributesW,
    },
    System::SystemServices::FILE_PERSISTENT_ACLS,
    UI::Shell::{
        FO_DELETE, FOF_ALLOWUNDO, FOF_NOERRORUI, FOF_SILENT, FOF_WANTNUKEWARNING, SHFILEOPSTRUCTW,
        SHFileOperationW,
    },
};

fn wide_path(path: &Path, verbatim: bool) -> io::Result<Vec<u16>> {
    let absolute = std::path::absolute(path)?;
    if absolute.as_os_str().encode_wide().any(|u| u == 0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "embedded NUL in path",
        ));
    }
    let display = vel_config::paths::display_path(&absolute);
    let path = if verbatim && absolute.as_os_str().encode_wide().count() >= 248 {
        // std::path::absolute has resolved dot components before adding the verbatim prefix.
        match display.components().next() {
            Some(std::path::Component::Prefix(prefix)) => match prefix.kind() {
                Prefix::Disk(_) => {
                    let mut out = OsString::from(r"\\?\");
                    out.push(display.as_os_str());
                    out
                }
                Prefix::UNC(..) => {
                    let mut out = Vec::from([
                        b'\\' as u16,
                        b'\\' as u16,
                        b'?' as u16,
                        b'\\' as u16,
                        b'U' as u16,
                        b'N' as u16,
                        b'C' as u16,
                        b'\\' as u16,
                    ]);
                    out.extend(display.as_os_str().encode_wide().skip(2));
                    return Ok(out.into_iter().chain([0]).collect());
                }
                _ => absolute.into_os_string(),
            },
            _ => absolute.into_os_string(),
        }
    } else if verbatim {
        absolute.into_os_string()
    } else {
        display.into_owned().into_os_string()
    };
    Ok(path.encode_wide().chain([0]).collect())
}

pub(super) fn move_file(from: &Path, to: &Path, replace: bool) -> io::Result<()> {
    let from = wide_path(from, true)?;
    let to = wide_path(to, true)?;
    let flags = MOVEFILE_WRITE_THROUGH
        | if replace {
            MOVEFILE_REPLACE_EXISTING
        } else {
            0
        };
    // SAFETY: both paths are NUL-terminated and live across the call. Omitting
    // COPY_ALLOWED makes cross-volume moves fail without touching either file.
    if unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), flags) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

pub(super) fn publish(stage: &Path, target: &Path, replace: bool) -> io::Result<()> {
    let name = wide_path(stage, true)?;
    // SAFETY: clear tempfile's TEMPORARY attribute before making the stage visible.
    if unsafe { SetFileAttributesW(name.as_ptr(), FILE_ATTRIBUTE_NORMAL) } == 0 {
        return Err(io::Error::last_os_error());
    }
    move_file(stage, target, replace)
}

struct Descriptor(PSECURITY_DESCRIPTOR);
impl Drop for Descriptor {
    fn drop(&mut self) {
        // SAFETY: GetSecurityInfo allocated this descriptor with LocalAlloc.
        unsafe { LocalFree(self.0) };
    }
}

pub(super) fn copy_dacl(source: &File, staged: &Path) -> io::Result<()> {
    let mut flags = 0;
    // SAFETY: the source remains open and the optional outputs not requested are null.
    if unsafe {
        GetVolumeInformationByHandleW(
            source.as_raw_handle(),
            null_mut(),
            0,
            null_mut(),
            null_mut(),
            &mut flags,
            null_mut(),
            0,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    if flags & FILE_PERSISTENT_ACLS == 0 {
        return Ok(());
    }
    let mut dacl: *mut ACL = null_mut();
    let mut descriptor = Descriptor(null_mut());
    // SAFETY: outputs point to writable pointers; the DACL is owned by the returned descriptor.
    let result = unsafe {
        GetSecurityInfo(
            source.as_raw_handle(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            null_mut(),
            null_mut(),
            &mut dacl,
            null_mut(),
            &mut descriptor.0,
        )
    };
    if result != ERROR_SUCCESS {
        return Err(io::Error::from_raw_os_error(result as i32));
    }
    let (mut control, mut revision) = (0, 0);
    // SAFETY: the successful security query populated descriptor; both outputs are live.
    if unsafe { GetSecurityDescriptorControl(descriptor.0, &mut control, &mut revision) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let protection = if control & SE_DACL_PROTECTED != 0 {
        PROTECTED_DACL_SECURITY_INFORMATION
    } else {
        UNPROTECTED_DACL_SECURITY_INFORMATION
    };
    let name = wide_path(staged, true)?;
    // SAFETY: the DACL (which may be null) remains owned by descriptor through this call.
    let result = unsafe {
        SetNamedSecurityInfoW(
            name.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | protection,
            null_mut(),
            null_mut(),
            dacl,
            null_mut(),
        )
    };
    if result != ERROR_SUCCESS {
        return Err(io::Error::from_raw_os_error(result as i32));
    }
    Ok(())
}

/// Move an existing local path to the Windows Recycle Bin. The shell must ask
/// separately before a permanent delete when the volume cannot recycle it.
pub fn trash(path: &Path) -> io::Result<()> {
    let mut path = wide_path(path, false)?;
    // The shell accepts wildcard lists and legacy MAX_PATH names; never silently
    // interpret a pattern or a truncated name as a request to delete.
    if path.len() >= 260
        || path
            .iter()
            .any(|&u| u == u16::from(b'*') || u == u16::from(b'?'))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "path is not supported by the Recycle Bin",
        ));
    }
    path.push(0);
    let mut operation = SHFILEOPSTRUCTW {
        wFunc: FO_DELETE,
        pFrom: path.as_ptr(),
        fFlags: (FOF_ALLOWUNDO | FOF_WANTNUKEWARNING | FOF_SILENT | FOF_NOERRORUI) as u16,
        ..Default::default()
    };
    // SAFETY: pFrom points at one double-NUL-terminated absolute path and the
    // operation structure remains writable through the call.
    let result = unsafe { SHFileOperationW(&mut operation) };
    let aborted = operation.fAnyOperationsAborted != 0;
    if result != 0 || aborted {
        return Err(io::Error::other(format!(
            "Recycle Bin operation failed or was cancelled ({result:#x})"
        )));
    }
    Ok(())
}
