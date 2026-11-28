//! Only fixed buffers and POSIX async-signal-safe calls are used in `handle`.
#[cfg(unix)]
use std::{
    fs, io,
    os::fd::IntoRawFd,
    path::Path,
    sync::{
        Mutex,
        atomic::{AtomicI32, AtomicU8, AtomicUsize, Ordering},
    },
};

#[cfg(unix)]
static ACTIVE: Mutex<Option<std::path::PathBuf>> = Mutex::new(None);
#[cfg(unix)]
static FD: AtomicI32 = AtomicI32::new(-1);
#[cfg(unix)]
const CAPACITY: usize = 512 * 1024;
#[cfg(unix)]
static CONTEXT: [AtomicU8; CAPACITY] = [const { AtomicU8::new(0) }; CAPACITY];
#[cfg(unix)]
static LENGTH: AtomicUsize = AtomicUsize::new(0);
#[cfg(all(unix, test))]
static TEST_TERMINATION_SIGNAL: AtomicI32 = AtomicI32::new(0);

#[cfg(unix)]
thread_local! {
    static ALT_STACK: std::cell::RefCell<Option<AltStack>> = const { std::cell::RefCell::new(None) };
}

#[cfg(unix)]
struct AltStack {
    prior: libc::stack_t,
    _storage: Box<[u8]>,
}

#[cfg(unix)]
impl Drop for AltStack {
    fn drop(&mut self) {
        // SAFETY: this TLS destructor runs on the thread that owns this stack.
        if unsafe { libc::sigaltstack(&self.prior, std::ptr::null_mut()) } != 0 {
            // Can't drop memory the kernel might still use as a stack.
            let _ = Box::leak(std::mem::take(&mut self._storage));
        }
    }
}

#[cfg(unix)]
pub(crate) fn install_thread_stack() -> io::Result<()> {
    ALT_STACK.with_borrow_mut(|stack| {
        // SAFETY: stack_t is a plain C struct; sigaltstack fills it on success.
        let mut prior: libc::stack_t = unsafe { std::mem::zeroed() };
        // SAFETY: prior is writable and null leaves the current stack unchanged.
        if unsafe { libc::sigaltstack(std::ptr::null(), &mut prior) } != 0 {
            return Err(io::Error::last_os_error());
        }
        if prior.ss_flags & libc::SS_DISABLE == 0 {
            return Ok(()); // The Rust runtime or host already installed one.
        }
        let mut storage = vec![0; 64 * 1024].into_boxed_slice();
        let alternate = libc::stack_t {
            ss_sp: storage.as_mut_ptr().cast(),
            ss_size: storage.len(),
            ss_flags: 0,
        };
        // SAFETY: storage stays pinned in TLS for the life of this registration.
        if unsafe { libc::sigaltstack(&alternate, std::ptr::null_mut()) } != 0 {
            return Err(io::Error::last_os_error());
        }
        *stack = Some(AltStack {
            prior,
            _storage: storage,
        });
        Ok(())
    })
}

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
    install_thread_stack()?;
    // SAFETY: cleanup is a static C callback, called only by normal process exit.
    if unsafe { libc::atexit(remove_reservation) } != 0 {
        return Err(io::Error::other("could not register crash-report cleanup"));
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
        action.sa_flags = libc::SA_RESETHAND | libc::SA_ONSTACK;
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
            // SAFETY: the descriptor was transferred from the File above.
            unsafe { libc::close(fd) };
            let _ = fs::remove_file(path);
            return Err(error);
        }
        installed.push((signal, previous));
    }
    *active = Some(path.into());
    Ok(())
}

#[cfg(unix)]
extern "C" fn remove_reservation() {
    if let Ok(active) = ACTIVE.try_lock()
        && let Some(path) = active.as_ref()
        && fs::symlink_metadata(path).is_ok_and(|m| m.is_file() && m.len() == 0)
    {
        let _ = fs::remove_file(path);
    }
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
    // Deliver the original fatal signal to this thread so the OS sees a crash,
    // not an ordinary exit. Explicitly restore SIG_DFL even for signals for
    // which the platform does not honor SA_RESETHAND.
    // https://developer.apple.com/documentation/xcode/interpreting-the-json-format-of-a-crash-report
    #[cfg(test)]
    let signal = match TEST_TERMINATION_SIGNAL.load(Ordering::Relaxed) {
        0 => signal,
        test => test,
    };
    // SAFETY: sigaction uses a live initialized record. The POSIX calls here
    // are async-signal-safe; a blocked signal is delivered after sigreturn.
    unsafe {
        let mut action: libc::sigaction = std::mem::zeroed();
        action.sa_sigaction = libc::SIG_DFL;
        libc::sigemptyset(&mut action.sa_mask);
        libc::sigaction(signal, &action, std::ptr::null_mut());
        if libc::raise(signal) != 0 {
            libc::_exit(128 + signal);
        }
    }
}

#[cfg(all(test, unix))]
#[test]
fn fatal_handler_writes_for_segv_and_worker_stack_overflow() {
    use std::{
        os::unix::process::ExitStatusExt,
        process::Command,
        time::{Duration, Instant},
    };
    if let Some(path) = std::env::var_os("VELOCITY_HANDLER_TEST_PATH") {
        install(Path::new(&path), "retained fixture context\n").unwrap();
        // In the macOS coding sandbox even a bare process killed by SIGSEGV
        // cannot be waited on. The handler writes for the real fault, then
        // terminates by a default-action signal that does not request an IPS report.
        TEST_TERMINATION_SIGNAL.store(libc::SIGTERM, Ordering::Relaxed);
        if std::env::var_os("VELOCITY_HANDLER_TEST_OVERFLOW").is_some() {
            #[allow(unconditional_recursion)]
            #[inline(never)]
            fn overflow(depth: usize) -> usize {
                let bytes = std::hint::black_box([depth as u8; 8192]);
                overflow(depth + 1) + std::hint::black_box(usize::from(bytes[0]))
            }
            std::thread::Builder::new()
                .stack_size(128 * 1024)
                .spawn(|| {
                    install_thread_stack().unwrap();
                    std::hint::black_box(overflow(0))
                })
                .unwrap()
                .join()
                .unwrap();
        } else {
            // SAFETY: the isolated child intentionally raises its own signal.
            unsafe { libc::raise(libc::SIGSEGV) };
        }
        unreachable!();
    }
    let directory = tempfile::tempdir().unwrap();
    for overflow in [false, true] {
        let path = directory.path().join(if overflow {
            "overflow.signal"
        } else {
            "segv.signal"
        });
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "signals::fatal_handler_writes_for_segv_and_worker_stack_overflow",
            ])
            .env("VELOCITY_HANDLER_TEST_PATH", &path);
        if overflow {
            command.env("VELOCITY_HANDLER_TEST_OVERFLOW", "1");
        }
        let mut child = command.spawn().unwrap();
        // Hang guard only: the fixture normally exits at once.
        let deadline = Instant::now() + Duration::from_secs(120);
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                panic!("crash fixture did not exit (overflow={overflow})");
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        assert_eq!(status.signal(), Some(libc::SIGTERM));
        let text = fs::read_to_string(path).unwrap();
        assert!(text.contains("retained fixture context"));
        assert!(text.contains("signal: SIGSEGV") || (overflow && text.contains("signal: SIGBUS")));
    }
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
