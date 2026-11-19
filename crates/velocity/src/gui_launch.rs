//! Launch the GUI independently of the terminal client.
use std::{
    fs::{self, DirBuilder, File, OpenOptions},
    io::{self, Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{
            fs::{DirBuilderExt, OpenOptionsExt},
            process::CommandExt,
        },
    },
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

const LOG_LIMIT: u64 = 1024 * 1024;
const GUI: &str = "VELOCITY_GUI_PROCESS";
const STDERR: &str = "VELOCITY_GUI_STDERR";

pub struct Startup {
    pub claim: Option<std::ffi::OsString>,
    pub log: Option<PathBuf>,
}
impl Startup {
    /// Called only at executable startup, before starting any threads.
    pub fn take() -> Self {
        use crate::instance::INHERITED_LOCK;
        let detached = std::env::var_os(GUI).is_some();
        let startup = Self {
            claim: detached.then(|| std::env::var_os(INHERITED_LOCK)).flatten(),
            log: std::env::var_os(STDERR).map(PathBuf::from),
        };
        // SAFETY: main calls this before reading options or starting any threads.
        unsafe {
            for marker in [GUI, STDERR, INHERITED_LOCK] {
                std::env::remove_var(marker);
            }
        }
        startup
    }
}

fn open_log(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .append(true)
        .create(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .and_then(|file| {
            if file.metadata()?.is_file() {
                Ok(file)
            } else {
                Err(io::Error::other("GUI log is not a regular file"))
            }
        })
}

fn rotate(path: &Path) -> io::Result<File> {
    fs::rename(path, path.with_extension("previous.log"))?;
    open_log(path)
}

pub fn log_path(endpoint: &Path) -> io::Result<PathBuf> {
    let dir = std::env::var_os("VELOCITY_LOG_DIR")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|home| PathBuf::from(home).join("Library/Logs/Velocity"))
        })
        .ok_or_else(|| io::Error::other("GUI logging needs HOME or VELOCITY_LOG_DIR"))?;
    let dir = std::path::absolute(dir)?;
    DirBuilder::new().recursive(true).mode(0o700).create(&dir)?;
    let profile = endpoint
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("default");
    Ok(dir.join(format!("gui-{profile}.log")))
}

pub fn gui_command(exe: &Path, log: io::Result<PathBuf>) -> io::Result<Command> {
    let log = log.and_then(|path| {
        let mut file = open_log(&path)?;
        if file.metadata()?.len() >= LOG_LIMIT {
            file = rotate(&path)?;
        }
        Ok((path, file))
    });
    let mut command = Command::new(exe);
    command
        .current_dir("/")
        .env(GUI, "1")
        .stdin(Stdio::null())
        .stdout(Stdio::null());
    match log {
        Ok((path, file)) => command.env(STDERR, path).stderr(file),
        Err(error) => {
            eprintln!("velocity: GUI log unavailable; using /dev/null: {error}");
            command.env_remove(STDERR).stderr(Stdio::null())
        }
    };
    // https://git-scm.com/docs/git-rev-parse#Documentation/git-rev-parse.txt---local-env-vars
    // Strip repository-local Git state, including the indexed config pairs, but
    // retain personal Git settings such as GIT_CONFIG_GLOBAL and GIT_SSH_COMMAND.
    for name in [
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        "GIT_CONFIG",
        "GIT_CONFIG_PARAMETERS",
        "GIT_CONFIG_COUNT",
        "GIT_OBJECT_DIRECTORY",
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_IMPLICIT_WORK_TREE",
        "GIT_GRAFT_FILE",
        "GIT_INDEX_FILE",
        "GIT_NO_REPLACE_OBJECTS",
        "GIT_REPLACE_REF_BASE",
        "GIT_PREFIX",
        "GIT_SHALLOW_FILE",
        "GIT_COMMON_DIR",
        "GIT_NAMESPACE",
        "GIT_QUARANTINE_PATH",
    ] {
        command.env_remove(name);
    }
    for (name, _) in std::env::vars_os() {
        if name
            .to_str()
            .is_some_and(|s| s.starts_with("GIT_CONFIG_KEY_") || s.starts_with("GIT_CONFIG_VALUE_"))
        {
            command.env_remove(name);
        }
    }
    // Snapshot first: no directory iteration or allocation after fork. Newly
    // created stdio and the Command exec-error pipe already use close-on-exec.
    let inherited_fds: Vec<i32> = fs::read_dir("/dev/fd")?
        .filter_map(|item| item.ok()?.file_name().to_str()?.parse().ok())
        .filter(|&fd| fd > libc::STDERR_FILENO)
        .collect();
    // Apple setsid(2): setsid creates the session AND process group. Calling
    // process_group(0) first makes the child a group leader and setsid fails.
    // https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/setsid.2.html
    // SAFETY: the callback only calls async-signal-safe POSIX functions.
    unsafe {
        command.pre_exec(move || {
            if libc::setsid() == -1 {
                return Err(io::Error::last_os_error());
            }
            for &fd in &inherited_fds {
                if libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) == -1 {
                    let error = io::Error::last_os_error();
                    if error.raw_os_error() != Some(libc::EBADF) {
                        return Err(error);
                    }
                }
            }
            // Claim::pass_to adds the following pre_exec hook to preserve only its lock.
            Ok(())
        });
    }
    Ok(command)
}

pub fn capture_stderr(path: PathBuf) -> io::Result<()> {
    let file = open_log(&path)?;
    let mut fds = [-1; 2];
    // SAFETY: pipe writes two descriptors; both are owned on success.
    let (reader, writer) = unsafe {
        if libc::pipe(fds.as_mut_ptr()) < 0 {
            return Err(io::Error::last_os_error());
        }
        (File::from_raw_fd(fds[0]), File::from_raw_fd(fds[1]))
    };
    // SAFETY: the descriptors are live; neither private end should survive exec.
    unsafe {
        for fd in fds {
            if libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) < 0 {
                return Err(io::Error::last_os_error());
            }
        }
    }
    std::thread::Builder::new()
        .name("velocity-gui-log".into())
        .spawn(move || {
            // Keep draining on a log error: do not let a full disk stall the editor.
            let mut reader = reader;
            if write_rotated(&mut reader, file, &path).is_err() {
                let _ = io::copy(&mut reader, &mut io::sink());
            }
        })?;
    // SAFETY: writer remains open across dup2; fd 2 is the process stderr.
    if unsafe { libc::dup2(writer.as_raw_fd(), libc::STDERR_FILENO) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn write_rotated(mut reader: impl Read, mut file: File, path: &Path) -> io::Result<()> {
    let mut len = file.metadata()?.len();
    let mut buf = [0; 8192];
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            return Ok(());
        }
        if len + n as u64 > LOG_LIMIT {
            file = rotate(path)?;
            len = 0;
        }
        file.write_all(&buf[..n])?;
        len += n as u64;
    }
}

extern "C" fn interrupt_wait(_: libc::c_int) {
    let message = b"velocity: wait interrupted; the editor is still running\n";
    // SAFETY: write and _exit are async-signal-safe, and message is static.
    unsafe {
        libc::write(libc::STDERR_FILENO, message.as_ptr().cast(), message.len());
        libc::_exit(130);
    }
}

pub fn install_client_interrupt() -> io::Result<()> {
    // SAFETY: sigaction is initialized with an empty mask and a C signal handler.
    unsafe {
        let mut action: libc::sigaction = std::mem::zeroed();
        libc::sigemptyset(&mut action.sa_mask);
        action.sa_sigaction = interrupt_wait as *const () as usize;
        if libc::sigaction(libc::SIGINT, &action, std::ptr::null_mut()) < 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::BufRead;

    #[test]
    fn gui_builder_detaches_shell_and_rotates_fixture_logs() {
        let tmp = tempfile::tempdir().unwrap();
        let log = tmp.path().join("gui.log");
        let mut command = gui_command(Path::new("/bin/sh"), Ok(log.clone())).unwrap();
        assert_eq!(command.get_current_dir(), Some(Path::new("/")));
        for name in [
            "GIT_DIR",
            "GIT_INDEX_FILE",
            "GIT_WORK_TREE",
            "GIT_COMMON_DIR",
        ] {
            assert!(
                command
                    .get_envs()
                    .any(|(key, value)| key == name && value.is_none())
            );
        }
        let mut child = command
            .stdout(Stdio::piped())
            .stdin(Stdio::piped())
            .args(["-c", "pwd; read reply"])
            .spawn()
            .unwrap();
        let mut cwd = String::new();
        io::BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut cwd)
            .unwrap();
        assert_eq!(cwd.trim(), "/");
        let pid = child.id() as i32;
        // SAFETY: query the still-running shell and this process; no signal is sent.
        unsafe {
            assert_eq!(libc::getpgid(pid), pid);
            assert_eq!(libc::getsid(pid), pid);
            assert_ne!(libc::getsid(0), pid);
        }
        child.stdin.take().unwrap().write_all(b"done\n").unwrap();
        assert!(child.wait().unwrap().success());
        let bytes = io::repeat(b'x').take(LOG_LIMIT * 3 + 1);
        write_rotated(bytes, open_log(&log).unwrap(), &log).unwrap();
        assert!(fs::metadata(&log).unwrap().len() <= LOG_LIMIT);
        assert_eq!(
            fs::metadata(log.with_extension("previous.log"))
                .unwrap()
                .len(),
            LOG_LIMIT
        );
    }

    #[test]
    fn gui_builder_launches_with_stderr_discarded_when_logging_fails() {
        let tmp = tempfile::tempdir().unwrap();
        let not_a_directory = tmp.path().join("blocked");
        fs::write(&not_a_directory, "file").unwrap();
        let cannot_rotate = tmp.path().join("full.log");
        open_log(&cannot_rotate)
            .unwrap()
            .set_len(LOG_LIMIT)
            .unwrap();
        fs::create_dir(cannot_rotate.with_extension("previous.log")).unwrap();
        for log in [
            Err(io::Error::from_raw_os_error(libc::EACCES)),
            Ok(not_a_directory.join("gui.log")),
            Ok(cannot_rotate),
        ] {
            let mut command = gui_command(Path::new("/bin/sh"), log).unwrap();
            assert!(
                command
                    .get_envs()
                    .any(|(key, value)| key == STDERR && value.is_none())
            );
            let output = command
                .env("HOME", tmp.path())
                .env("SHELL", "/bin/sh")
                .stdout(Stdio::piped())
                .args(["-c", "printf started; printf 'discard me' >&2"])
                .output()
                .unwrap();
            assert!(output.status.success());
            assert_eq!(output.stdout, b"started");
            assert!(output.stderr.is_empty());
        }
    }

    #[test]
    fn detached_gui_does_not_hold_a_jobserver_descriptor() {
        let tmp = tempfile::tempdir().unwrap();
        let jobserver = File::open(tmp.path()).unwrap();
        // SAFETY: duplicate the file as a high, non-close-on-exec descriptor.
        let fd = unsafe { libc::fcntl(jobserver.as_raw_fd(), libc::F_DUPFD, 101) };
        assert!(fd >= 101);
        // SAFETY: fd is newly owned.
        let inherited = unsafe { File::from_raw_fd(fd) };
        let mut child = gui_command(Path::new("/bin/sh"), Ok(tmp.path().join("gui.log"))).unwrap();
        assert!(
            child
                .env("HOME", tmp.path())
                .env("SHELL", "/bin/sh")
                .args(["-c", "test ! -e /dev/fd/$1", "fixture", &fd.to_string()])
                .status()
                .unwrap()
                .success()
        );
        assert!(inherited.metadata().is_ok());
    }
}
