//! Only fixed buffers and POSIX async-signal-safe calls are used in `handle`.
#[cfg(unix)]
use std::{
    fs,
    os::fd::IntoRawFd,
    sync::atomic::{AtomicI32, AtomicU8, AtomicUsize, Ordering},
};
use std::{io, path::Path, sync::Mutex};

static ACTIVE: Mutex<Option<std::path::PathBuf>> = Mutex::new(None);
#[cfg(unix)]
static FD: AtomicI32 = AtomicI32::new(-1);
#[cfg(unix)]
const CAPACITY: usize = 512 * 1024;
#[cfg(unix)]
static CONTEXT: [AtomicU8; CAPACITY] = [const { AtomicU8::new(0) }; CAPACITY];
#[cfg(unix)]
static LENGTH: AtomicUsize = AtomicUsize::new(0);

#[cfg(unix)]
pub(crate) fn publish(context: &str) {
    // Called with the reporter log mutex held: there is one publisher. A signal arriving
    // during a publication sees length zero and writes only the fixed emergency header.
    LENGTH.store(0, Ordering::SeqCst);
    for (destination, source) in CONTEXT.iter().zip(context.bytes()) {
        destination.store(source, Ordering::SeqCst);
    }
    LENGTH.store(context.len().min(CAPACITY), Ordering::SeqCst);
}
#[cfg(not(unix))]
pub(crate) fn publish(_: &str) {}

#[cfg(unix)]
pub(crate) fn install(path: &Path, context: &str) -> io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut active = ACTIVE.lock().unwrap_or_else(|e| e.into_inner());
    if active.is_some() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "fatal signal handlers already installed",
        ));
    }
    let file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    // Retained until process exit so a concurrent signal can never write to a recycled fd.
    let fd = file.into_raw_fd();
    FD.store(fd, Ordering::SeqCst);
    publish(context);
    let mut installed = Vec::new();
    for signal in [
        libc::SIGABRT,
        libc::SIGBUS,
        libc::SIGFPE,
        libc::SIGILL,
        libc::SIGSEGV,
    ] {
        // SAFETY: sigaction is a plain C record; all-zero initialization is valid before
        // setting the handler and initializing its signal mask with sigemptyset.
        let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
        action.sa_sigaction = handle as *const () as usize;
        action.sa_flags = libc::SA_RESETHAND;
        // SAFETY: action's mask is live writable storage for sigemptyset.
        unsafe { libc::sigemptyset(&mut action.sa_mask) };
        // SAFETY: previous is only read after a successful sigaction initializes it.
        let mut previous: libc::sigaction = unsafe { std::mem::zeroed() };
        // SAFETY: both records live across the call; the handler has the POSIX C ABI.
        if unsafe { libc::sigaction(signal, &action, &mut previous) } != 0 {
            let error = io::Error::last_os_error();
            for (signal, previous) in installed {
                // SAFETY: restore exactly the actions initialized by successful calls above.
                unsafe { libc::sigaction(signal, &previous, std::ptr::null_mut()) };
            }
            FD.store(-1, Ordering::SeqCst);
            let _ = fs::remove_file(path);
            return Err(error);
        }
        installed.push((signal, previous));
    }
    *active = Some(path.into());
    Ok(())
}

#[cfg(unix)]
extern "C" fn handle(signal: libc::c_int) {
    let fd = FD.load(Ordering::SeqCst);
    if fd >= 0 {
        raw_write(fd, b"Velocity fatal signal report\n");
        let length = LENGTH.load(Ordering::SeqCst).min(CAPACITY);
        let mut buffer = [0u8; 512];
        for offset in (0..length).step_by(buffer.len()) {
            let count = buffer.len().min(length - offset);
            for i in 0..count {
                buffer[i] = CONTEXT[offset + i].load(Ordering::SeqCst);
            }
            raw_write(fd, &buffer[..count]);
        }
        raw_write(
            fd,
            match signal {
                libc::SIGABRT => b"signal: SIGABRT\n",
                libc::SIGBUS => b"signal: SIGBUS\n",
                libc::SIGFPE => b"signal: SIGFPE\n",
                libc::SIGILL => b"signal: SIGILL\n",
                _ => b"signal: SIGSEGV\n",
            },
        );
        raw_write(fd, b"backtrace: unavailable in async signal context\n");
    }
    // SAFETY: _exit is async-signal-safe and never unwinds through the interrupted code.
    unsafe { libc::_exit(128 + signal) }
}
#[cfg(unix)]
fn raw_write(fd: libc::c_int, mut bytes: &[u8]) {
    while !bytes.is_empty() {
        // SAFETY: the slice is live for this call; write is POSIX async-signal-safe.
        let count = unsafe { libc::write(fd, bytes.as_ptr().cast(), bytes.len()) };
        if count <= 0 {
            break;
        }
        bytes = &bytes[count as usize..];
    }
}
