//! Win32 launch with an inherited-handle list and job membership at creation.
use crate::Exec;
use std::{
    collections::BTreeMap,
    ffi::{OsStr, OsString},
    fs::File,
    io,
    os::windows::{
        ffi::{OsStrExt, OsStringExt},
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
        process::ExitStatusExt,
    },
    path::{Path, PathBuf},
    process::ExitStatus,
    ptr::{null, null_mut},
};
use windows_sys::Win32::{
    Foundation::{
        ERROR_BROKEN_PIPE, ERROR_PIPE_NOT_CONNECTED, HANDLE, HANDLE_FLAG_INHERIT,
        SetHandleInformation, WAIT_OBJECT_0, WAIT_TIMEOUT,
    },
    Security::SECURITY_ATTRIBUTES,
    System::{
        JobObjects::*,
        Pipes::{CreatePipe, PeekNamedPipe},
        SystemInformation::GetSystemDirectoryW,
        Threading::*,
    },
};

#[derive(Debug)]
pub(super) struct Job(OwnedHandle);
impl Job {
    pub(super) fn new() -> io::Result<Self> {
        // SAFETY: a null name and null attributes request a private non-inheritable job.
        let handle = unsafe { CreateJobObjectW(null(), null()) };
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: CreateJobObjectW returned a fresh owned handle.
        let handle = unsafe { OwnedHandle::from_raw_handle(handle) };
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        // SAFETY: the input points to a fully initialized limits struct of the declared type.
        if unsafe {
            SetInformationJobObject(
                handle.as_raw_handle(),
                JobObjectExtendedLimitInformation,
                (&raw const limits).cast(),
                size_of_val(&limits) as u32,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(Self(handle))
    }

    pub(super) fn kill(&self) {
        // SAFETY: the handle stays alive; only processes in this build's job are terminated.
        unsafe { TerminateJobObject(self.0.as_raw_handle(), 1) };
    }
}

#[derive(Debug)]
pub(super) struct Child {
    process: OwnedHandle,
    pub(super) stdin: Option<File>,
    pub(super) stdout: Option<File>,
    pub(super) stderr: Option<File>,
}
impl Child {
    fn wait_for(&self, timeout: u32) -> io::Result<Option<ExitStatus>> {
        let handle = self.process.as_raw_handle();
        // SAFETY: the process handle stays valid throughout the wait.
        match unsafe { WaitForSingleObject(handle, timeout) } {
            WAIT_OBJECT_0 => {
                let mut code = 0;
                // SAFETY: code is writable and the process is signaled.
                if unsafe { GetExitCodeProcess(handle, &mut code) } == 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(Some(ExitStatus::from_raw(code)))
            }
            WAIT_TIMEOUT => Ok(None),
            _ => Err(io::Error::last_os_error()),
        }
    }
    pub(super) fn try_wait(&self) -> io::Result<Option<ExitStatus>> {
        self.wait_for(0)
    }
    pub(super) fn wait(&self) -> io::Result<ExitStatus> {
        self.wait_for(INFINITE)
            .map(|status| status.expect("infinite wait"))
    }
}

fn wide(s: &OsStr) -> io::Result<Vec<u16>> {
    let mut value: Vec<u16> = s.encode_wide().collect();
    if value.contains(&0) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "embedded NUL"));
    }
    value.push(0);
    Ok(value)
}

fn system_dir() -> io::Result<PathBuf> {
    let mut buffer = vec![0u16; 32768];
    // SAFETY: buffer points to the specified number of writable code units.
    let len = unsafe { GetSystemDirectoryW(buffer.as_mut_ptr(), buffer.len() as u32) } as usize;
    if len == 0 || len >= buffer.len() {
        return Err(io::Error::last_os_error());
    }
    Ok(OsString::from_wide(&buffer[..len]).into())
}

fn executable(program: &str, cwd: &Path, env: &BTreeMap<String, String>) -> io::Result<PathBuf> {
    let program = Path::new(program);
    let find = |candidate: PathBuf| {
        if candidate.extension().is_some() {
            return candidate.is_file().then_some(candidate);
        }
        env.get("PATHEXT")
            .map_or(".EXE;.COM;.BAT;.CMD", String::as_str)
            .split(';')
            .filter_map(|ext| ext.strip_prefix('.'))
            .filter(|ext| !ext.is_empty() && !ext.contains(['/', '\\', ':']))
            .map(|ext| candidate.with_extension(ext))
            .find(|path| path.is_file())
    };
    let result = if program.components().count() != 1 || program.has_root() {
        find(cwd.join(program))
    } else {
        env.get("PATH")
            .into_iter()
            .flat_map(std::env::split_paths)
            .map(|dir| cwd.join(dir).join(program))
            .find_map(find)
            .or_else(|| system_dir().ok().and_then(|dir| find(dir.join(program))))
    };
    result.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!("{} not found", program.display()),
        )
    })
}

fn push_argv(out: &mut OsString, argument: &OsStr) {
    // Always quote; duplicate backslashes before quotes and before the closing quote.
    let mut units = vec![u16::from(b'"')];
    let slash = u16::from(b'\\');
    let quote = u16::from(b'"');
    let mut slashes = 0;
    for c in argument.encode_wide() {
        if c == slash {
            slashes += 1;
            continue;
        }
        units.extend(std::iter::repeat_n(
            slash,
            if c == quote { slashes * 2 + 1 } else { slashes },
        ));
        slashes = 0;
        units.push(c);
    }
    units.extend(std::iter::repeat_n(slash, slashes * 2));
    units.push(quote);
    out.push(OsString::from_wide(&units));
}

fn shell_word(word: &str) -> io::Result<String> {
    if word.contains(['"', '%', '\r', '\n', '\0']) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "argument cannot be quoted safely for cmd.exe; use shell_cmd",
        ));
    }
    Ok(format!("\"{word}\""))
}

fn command_line(exec: &Exec, env: &BTreeMap<String, String>) -> io::Result<(PathBuf, OsString)> {
    let mut source = exec.shell_cmd.clone();
    if source.is_none() && exec.shell {
        source = Some(
            exec.cmd
                .iter()
                .map(|word| shell_word(word))
                .collect::<io::Result<Vec<_>>>()?
                .join(" "),
        );
    }
    let mut line = OsString::new();
    if source.is_none() {
        let program = exec.cmd.first().ok_or(io::ErrorKind::InvalidInput)?;
        let exe = executable(program, &exec.working_dir, env)?;
        let batch = exe
            .extension()
            .and_then(OsStr::to_str)
            .is_some_and(|ext| ext.eq_ignore_ascii_case("cmd") || ext.eq_ignore_ascii_case("bat"));
        if !batch {
            push_argv(&mut line, exe.as_os_str());
            for arg in &exec.cmd[1..] {
                line.push(" ");
                push_argv(&mut line, OsStr::new(arg));
            }
            return Ok((exe, line));
        }
        let mut args = vec![shell_word(&vel_config::paths::display(&exe))?];
        for arg in &exec.cmd[1..] {
            args.push(shell_word(arg)?);
        }
        source = Some(args.join(" "));
    }
    let cmd = system_dir()?.join("cmd.exe");
    push_argv(&mut line, cmd.as_os_str());
    line.push(" /D /V:OFF /S /C \"");
    line.push(source.expect("shell command"));
    line.push("\"");
    Ok((cmd, line))
}

fn pipe(child_reads: bool) -> io::Result<(File, File)> {
    let mut read = null_mut();
    let mut write = null_mut();
    let attributes = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: null_mut(),
        bInheritHandle: 1,
    };
    // SAFETY: both output pointers and the attributes are valid for this call.
    if unsafe { CreatePipe(&mut read, &mut write, &attributes, 0) } == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the two returned handles are freshly owned.
    let (read, write) = unsafe { (File::from_raw_handle(read), File::from_raw_handle(write)) };
    let (parent, child) = if child_reads {
        (write, read)
    } else {
        (read, write)
    };
    // SAFETY: only change the inherit bit on the live parent end.
    if unsafe { SetHandleInformation(parent.as_raw_handle(), HANDLE_FLAG_INHERIT, 0) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok((parent, child))
}

struct Attributes(Vec<usize>);
impl Attributes {
    fn new() -> io::Result<Self> {
        let mut size = 0;
        // SAFETY: null requests the required allocation size for two attributes.
        unsafe { InitializeProcThreadAttributeList(null_mut(), 2, 0, &mut size) };
        let mut allocation = vec![0usize; size.div_ceil(size_of::<usize>())];
        // SAFETY: the word-aligned allocation is at least size bytes.
        if unsafe {
            InitializeProcThreadAttributeList(allocation.as_mut_ptr().cast(), 2, 0, &mut size)
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(Self(allocation))
    }

    fn ptr(&mut self) -> LPPROC_THREAD_ATTRIBUTE_LIST {
        self.0.as_mut_ptr().cast()
    }

    fn set(&mut self, key: u32, handles: &[HANDLE]) -> io::Result<()> {
        // SAFETY: the list is initialized and the caller retains the handles and
        // their array until after CreateProcessW and deletion of this list.
        if unsafe {
            UpdateProcThreadAttribute(
                self.ptr(),
                0,
                key as usize,
                handles.as_ptr().cast(),
                size_of_val(handles),
                null_mut(),
                null(),
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}
impl Drop for Attributes {
    fn drop(&mut self) {
        // SAFETY: the list was initialized and has not yet been deleted.
        unsafe { DeleteProcThreadAttributeList(self.ptr()) };
    }
}

pub(super) fn spawn(exec: &Exec, env: &BTreeMap<String, String>, job: &Job) -> io::Result<Child> {
    let (exe, line) = command_line(exec, env)?;
    let exe = wide(exe.as_os_str())?;
    let mut line = wide(&line)?;
    let cwd = wide(exec.working_dir.as_os_str())?;
    // Key spelling is canonicalized by expand_environment. UTF-16 block is sorted
    // by name and double-NUL terminated, including when there are no entries.
    let mut block = Vec::new();
    for (key, value) in env {
        if key.is_empty() || key.contains('=') {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid environment variable name",
            ));
        }
        block.extend(wide(OsStr::new(&format!("{key}={value}")))?);
    }
    if block.is_empty() {
        block.push(0);
    }
    block.push(0);
    let (stdin, input) = pipe(true)?;
    let stdin = exec.interactive.then_some(stdin);
    let (stdout, output) = pipe(false)?;
    let (stderr, error) = pipe(false)?;
    let handles = [
        input.as_raw_handle(),
        output.as_raw_handle(),
        error.as_raw_handle(),
    ];
    let jobs = [job.0.as_raw_handle()];
    let mut attributes = Attributes::new()?;
    attributes.set(PROC_THREAD_ATTRIBUTE_HANDLE_LIST, &handles)?;
    // Unlike assigning after spawn, this covers descendants launched by the first instruction.
    attributes.set(PROC_THREAD_ATTRIBUTE_JOB_LIST, &jobs)?;
    let mut info = STARTUPINFOEXW::default();
    info.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
    info.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    [
        info.StartupInfo.hStdInput,
        info.StartupInfo.hStdOutput,
        info.StartupInfo.hStdError,
    ] = handles;
    info.lpAttributeList = attributes.ptr();
    let mut process = PROCESS_INFORMATION::default();
    // SAFETY: strings, environment, handles and both attribute arrays live across
    // the call. No handle other than the three child stdio ends is inherited.
    if unsafe {
        CreateProcessW(
            exe.as_ptr(),
            line.as_mut_ptr(),
            null(),
            null(),
            1,
            CREATE_NO_WINDOW | CREATE_UNICODE_ENVIRONMENT | EXTENDED_STARTUPINFO_PRESENT,
            block.as_ptr().cast(),
            cwd.as_ptr(),
            &info.StartupInfo,
            &mut process,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: both handles returned on success belong to the caller.
    let (process, _thread) = unsafe {
        (
            OwnedHandle::from_raw_handle(process.hProcess),
            OwnedHandle::from_raw_handle(process.hThread),
        )
    };
    Ok(Child {
        process,
        stdin,
        stdout: Some(stdout),
        stderr: Some(stderr),
    })
}

pub(super) fn available(pipe: HANDLE) -> io::Result<Option<usize>> {
    let mut bytes = 0;
    // SAFETY: querying one live read handle, with no data buffer requested.
    if unsafe { PeekNamedPipe(pipe, null_mut(), 0, null_mut(), &mut bytes, null_mut()) } != 0 {
        return Ok(Some(bytes as usize));
    }
    let error = io::Error::last_os_error();
    match error.raw_os_error().map(|e| e as u32) {
        Some(ERROR_BROKEN_PIPE | ERROR_PIPE_NOT_CONNECTED) => Ok(None),
        _ => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BuildSystem, Event, Stream, Task, Variables};
    use std::{
        fs,
        os::windows::process::CommandExt,
        process::Command,
        time::{Duration, Instant},
    };

    const MAX_WAIT: Duration = Duration::from_secs(60);
    const ROLE: &str = "VELOCITY_WIN_TEST_JOB_ROLE";
    const PID_FILE: &str = "VELOCITY_WIN_TEST_JOB_PID";
    const FIXTURE: &str = "process::windows::tests::job_fixture";

    #[test]
    fn job_fixture() {
        match std::env::var(ROLE).as_deref() {
            Ok("parent") => {
                let mut child = Command::new(std::env::current_exe().unwrap())
                    .args(["--exact", FIXTURE])
                    .env(ROLE, "child")
                    .creation_flags(CREATE_NO_WINDOW)
                    .spawn()
                    .unwrap();
                fs::write(std::env::var_os(PID_FILE).unwrap(), child.id().to_string()).unwrap();
                let _ = child.wait().unwrap();
            }
            Ok("child") => std::thread::sleep(MAX_WAIT * 2),
            _ => {}
        }
    }

    #[test]
    fn cancellation_kills_a_descendant_created_by_the_build() {
        let dir = tempfile::tempdir().unwrap();
        let pidfile = dir.path().join("descendant");
        let build = BuildSystem::from_value(
            "fixture",
            serde_json::json!({
                "cmd": [std::env::current_exe().unwrap(), "--exact", FIXTURE],
                "working_dir": dir.path(),
                "env": {(ROLE): "parent", (PID_FILE): pidfile}
            }),
        )
        .unwrap()
        .resolve(&Variables::default())
        .unwrap();
        let task = Task::spawn(&build).unwrap();
        let deadline = Instant::now() + MAX_WAIT;
        let pid = loop {
            if let Ok(pid) = fs::read_to_string(&pidfile)
                && let Ok(pid) = pid.parse::<u32>()
            {
                break pid;
            }
            assert!(
                !task.finished() && Instant::now() < deadline,
                "descendant never started"
            );
            std::thread::sleep(Duration::from_millis(20));
        };
        use windows_sys::Win32::Storage::FileSystem::SYNCHRONIZE;
        // SAFETY: the test descendant is still live and the handle is for a wait only.
        let child = unsafe { OpenProcess(SYNCHRONIZE, 0, pid) };
        assert!(!child.is_null(), "{}", io::Error::last_os_error());
        // SAFETY: OpenProcess returned a fresh owned handle.
        let child = unsafe { OwnedHandle::from_raw_handle(child) };
        task.cancel();
        // SAFETY: the child handle remains valid for the bounded wait.
        let waited =
            unsafe { WaitForSingleObject(child.as_raw_handle(), MAX_WAIT.as_millis() as u32) };
        assert_eq!(
            waited, WAIT_OBJECT_0,
            "descendant still running after cancelling the build"
        );
        let deadline = Instant::now() + MAX_WAIT;
        loop {
            if let Event::Finished { cancelled, .. } = task
                .events
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .unwrap()
            {
                assert!(cancelled);
                break;
            }
        }
    }

    #[test]
    fn path_lookup_and_env_overrides_drive_a_windows_shell() {
        let dir = tempfile::tempdir().unwrap();
        let filename = dir.path().join("with spaces.rs");
        fs::write(
            dir.path().join("probe.cmd"),
            b"@echo off\r\necho %VEL_TEST_VALUE%\r\necho %~1\r\n",
        )
        .unwrap();
        let variables = Variables::new(Some(&filename), None, None, None);
        let exec = BuildSystem::from_value("fixture", serde_json::json!({
            "cmd": ["probe", "$file"], "env": {"Path": "ignore-me", "vel_test_value": "port-ok"},
            "path": dir.path(),
        })).unwrap().resolve(&variables).unwrap();
        assert_eq!(exec.env["PATH"], dir.path().to_string_lossy());
        let task = Task::spawn(&exec).unwrap();
        let deadline = Instant::now() + MAX_WAIT;
        let mut text = String::new();
        loop {
            match task
                .events
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .unwrap()
            {
                Event::Output {
                    stream: Stream::Stdout,
                    text: chunk,
                } => text.push_str(&chunk),
                Event::OutputBatch(chunks) => {
                    for (stream, chunk) in chunks {
                        if stream == Stream::Stdout {
                            text.push_str(&chunk);
                        }
                    }
                }
                Event::Error(error) => panic!("{error}"),
                Event::Finished { code, .. } => {
                    assert_eq!(code, Some(0), "{text}");
                    break;
                }
                _ => {}
            }
        }
        assert!(text.lines().any(|line| line == "port-ok"), "{text}");
        assert!(
            text.lines().any(|line| line.ends_with("with spaces.rs")),
            "{text}"
        );
    }
}
