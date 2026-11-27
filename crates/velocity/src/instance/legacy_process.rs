//! On Darwin, a full UNIX listen queue can refuse connects like a stale inode.
use std::{ffi::OsStr, io, os::unix::ffi::OsStrExt, path::Path};

use super::socket_identity;

// These are the LP64 structures in the public SDK's <sys/proc_info.h>.
// https://github.com/apple-oss-distributions/xnu/blob/main/bsd/sys/proc_info.h
const PROC_PIDFDSOCKETINFO: i32 = 3;
const PROC_PIDFD_SOCKET: u32 = 2;
const SOCKINFO_UN: i32 = 3;

#[repr(C)]
struct SocketFdInfo {
    _file_and_stat: [u64; 23],
    family: i32,
    options: i16,
    _socket_fields: [u8; 66],
    kind: i32,
    _unix_fields: [u8; 20],
    address: libc::sockaddr_un,
    _rest: [u8; 406],
}

/// Called only after two failed connects. An inaccessible fd table in a
/// known editor is ambiguous: never unlink that editor's possible listener.
pub(super) fn has_listener(path: &Path, uid: u32, identity: (u64, u64)) -> bool {
    // SAFETY: a null buffer queries the count; the second call writes at
    // most the supplied byte capacity into the initialized pid array.
    let pids = unsafe {
        let limit = libc::proc_listallpids(std::ptr::null_mut(), 0);
        if limit <= 0 {
            return true;
        }
        let mut pids = vec![0_i32; limit as usize + 64];
        let count = libc::proc_listallpids(
            pids.as_mut_ptr().cast(),
            std::mem::size_of_val(pids.as_slice()) as i32,
        );
        if count <= 0 || count as usize >= pids.len() {
            return true;
        }
        pids.truncate(count as usize);
        pids
    };
    let executable = std::env::current_exe().ok();
    let mut fds = Vec::new();
    for pid in pids {
        if pid <= 0 || pid as u32 == std::process::id() {
            continue;
        }
        // SAFETY: libproc fills the given structures/path buffer synchronously.
        let (editor, same_user) = unsafe {
            let mut info: libc::proc_bsdinfo = std::mem::zeroed();
            if libc::proc_pidinfo(
                pid,
                libc::PROC_PIDTBSDINFO,
                0,
                (&raw mut info).cast(),
                std::mem::size_of_val(&info) as i32,
            ) != std::mem::size_of_val(&info) as i32
                || info.pbi_uid != uid
            {
                (false, false)
            } else {
                let mut name = [0_u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
                let length = libc::proc_pidpath(pid, name.as_mut_ptr().cast(), name.len() as u32);
                let editor = if length > 0 {
                    let name = name[..length as usize]
                        .strip_suffix(&[0])
                        .unwrap_or(&name[..length as usize]);
                    let exe = Path::new(OsStr::from_bytes(name));
                    Some(exe) == executable.as_deref()
                        || exe
                            .file_name()
                            .is_some_and(|name| name.as_bytes().eq_ignore_ascii_case(b"velocity"))
                } else {
                    // If the pathname is restricted, libproc's comm is enough
                    // to identify the possible editor conservatively.
                    info.pbi_comm
                        .map(|byte| byte as u8)
                        .split(|&byte| byte == 0)
                        .next()
                        .is_some_and(|name| name.eq_ignore_ascii_case(b"velocity"))
                };
                (editor, true)
            }
        };
        if same_user && listener(pid, &mut fds, path, uid, identity).unwrap_or(editor) {
            return true;
        }
    }
    false
}

fn listener(
    pid: i32,
    fds: &mut Vec<libc::proc_fdinfo>,
    path: &Path,
    uid: u32,
    identity: (u64, u64),
) -> Option<bool> {
    let size = std::mem::size_of::<libc::proc_fdinfo>();
    // SAFETY: all libproc writes have the size of their live output buffer.
    unsafe {
        let bytes = libc::proc_pidinfo(pid, libc::PROC_PIDLISTFDS, 0, std::ptr::null_mut(), 0);
        if gone(bytes) {
            return Some(false);
        }
        if bytes <= 0 || bytes as usize / size > 32_768 {
            return None;
        }
        fds.resize(bytes as usize / size + 32, std::mem::zeroed());
        let capacity = std::mem::size_of_val(fds.as_slice());
        let bytes = libc::proc_pidinfo(
            pid,
            libc::PROC_PIDLISTFDS,
            0,
            fds.as_mut_ptr().cast(),
            capacity as i32,
        );
        if gone(bytes) {
            return Some(false);
        }
        if bytes <= 0 || bytes as usize >= capacity {
            return None;
        }
        let mut complete = true;
        for fd in &fds[..bytes as usize / size] {
            if fd.proc_fdtype != PROC_PIDFD_SOCKET {
                continue;
            }
            let mut socket: SocketFdInfo = std::mem::zeroed();
            let bytes = libc::proc_pidfdinfo(
                pid,
                fd.proc_fd,
                PROC_PIDFDSOCKETINFO,
                (&raw mut socket).cast(),
                std::mem::size_of_val(&socket) as i32,
            );
            if bytes != std::mem::size_of_val(&socket) as i32 {
                let error = io::Error::last_os_error().raw_os_error();
                if error == Some(libc::ESRCH) {
                    return Some(false);
                }
                complete &= error == Some(libc::EBADF);
                continue;
            }
            if socket.family != libc::AF_UNIX
                || socket.kind != SOCKINFO_UN
                || i32::from(socket.options) & libc::SO_ACCEPTCONN == 0
            {
                continue;
            }
            let address = socket.address.sun_path.map(|byte| byte as u8);
            let bound = address.split(|&byte| byte == 0).next().unwrap_or_default();
            if !bound.is_empty()
                && (path.as_os_str().as_bytes() == bound
                    || socket_identity(Path::new(OsStr::from_bytes(bound)), uid).ok()
                        == Some(identity))
            {
                return Some(true);
            }
        }
        complete.then_some(false)
    }
}

/// A process that exited while it was being inspected has no descriptor table
/// left, so it cannot be the listener. Only an unreadable table of a live process
/// (for example EPERM) stays inconclusive and is treated conservatively.
fn gone(bytes: i32) -> bool {
    bytes == 0 || (bytes < 0 && io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_exited_process_is_not_a_listener() {
        let mut child = std::process::Command::new("/usr/bin/true").spawn().unwrap();
        let pid = child.id() as i32;
        child.wait().unwrap();
        let path = std::env::temp_dir().join("velocity-exited-process.sock");
        // SAFETY: getuid has no preconditions.
        let uid = unsafe { libc::getuid() };
        assert_eq!(
            listener(pid, &mut Vec::new(), &path, uid, (0, 0)),
            Some(false)
        );
    }
}
