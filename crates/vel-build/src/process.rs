//! Bounded UTF-8 streaming and Unix process-group cancellation.
use crate::Exec;
#[cfg(unix)]
use std::{io::Write, os::fd::AsRawFd, process::ChildStdin};
use std::{
    io::{self, Read},
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

/// Maximum UTF-8 bytes in a submitted input line, excluding the newline.
pub const MAX_INPUT_LINE: usize = 4096;
const INPUT_QUEUE: usize = 8;

/// Why an input line was not queued.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputError {
    /// The process is no longer accepting input.
    Closed,
    /// The child isn't keeping up. The caller may retry the same line.
    Full,
    /// The payload exceeded the line cap or contained a line break.
    InvalidLine,
}
/// Origin of an output chunk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stream {
    /// Standard output.
    Stdout,
    /// Standard error.
    Stderr,
    /// Echo of a submitted stdin line. It is not parsed for build diagnostics.
    Input,
}
/// Incremental output or final process result. Completion follows pipe drain, bounded when descendants retain a pipe.
#[derive(Debug)]
pub enum Event {
    /// Decoded text, including newline characters.
    Output {
        /// Origin.
        stream: Stream,
        /// Text with incomplete UTF-8 retained between reads.
        text: String,
    },
    /// A completed producer publishes its retained backlog in one UI update.
    OutputBatch(Vec<(Stream, String)>),
    /// Final worker-parsed totals, ordered before completion.
    Results(crate::Results),
    /// Old transport data was discarded at a line boundary.
    Truncated {
        /// Cumulative discarded bytes.
        bytes: usize,
        /// Absolute row of the first retained output line.
        first_line: usize,
        /// Cancellation discards the retained backlog; keep the output already delivered.
        cancelling: bool,
    },
    /// Nonfatal environment fallback; shown in the status bar.
    Warning(String),
    /// Worker/pipe error.
    Error(String),
    /// The child closed its stdin early, or the input pipe failed.
    InputClosed,
    /// Exit status and elapsed wall time.
    Finished {
        /// Process exit code (None after a signal).
        code: Option<i32>,
        /// POSIX signal that terminated the main process.
        signal: Option<i32>,
        /// Cancellation was requested.
        cancelled: bool,
        /// Spawn through pipe drain.
        elapsed: Duration,
    },
}
/// Asynchronous build handle. Drop cancels the complete process group.
#[derive(Debug)]
pub struct Task {
    /// Ordered events with bounded tail retention and priority cancellation.
    pub events: crate::output::Events,
    cancel: Arc<AtomicBool>,
    finished: Arc<AtomicBool>,
    pid: Arc<AtomicU32>,
    input: Option<mpsc::SyncSender<String>>,
    worker: Option<std::thread::JoinHandle<()>>,
}
impl Task {
    /// Spawns argv or shell_cmd with optional piped input and separate stdout/stderr pipes.
    pub fn spawn(exec: &Exec) -> Result<Self, String> {
        Self::spawn_after(exec, None)
    }
    /// Kills the predecessor group before starting its replacement; reaping stays asynchronous.
    pub fn spawn_after(exec: &Exec, previous: Option<Task>) -> Result<Self, String> {
        let exec = exec.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        let finished = Arc::new(AtomicBool::new(false));
        let pid = Arc::new(AtomicU32::new(0));
        let child_pid = pid.clone();
        let stop = cancel.clone();
        let (tx, events) =
            crate::output::channel(exec.parser.clone(), finished.clone(), cancel.clone());
        let (input, incoming) = if exec.interactive {
            let (sender, receiver) = mpsc::sync_channel(INPUT_QUEUE);
            (Some(sender), Some(receiver))
        } else {
            (None, None)
        };
        if let Some(old) = &previous {
            old.cancel();
        }
        let worker = std::thread::Builder::new()
            .name("velocity-build".into())
            .spawn(move || {
                // cancel() killed the published group synchronously. Dropping the old
                // receiver also releases backpressure; detached pipes must not delay restart.
                drop(previous);
                let at = Instant::now();
                let result = run(&exec, &tx, &stop, &child_pid, incoming);
                let (code, signal) = match result {
                    Ok(status) => status,
                    Err(e) => {
                        let _ = send(&tx, Event::Error(e), &stop);
                        (None, None)
                    }
                };
                // The receiver gives cancellation priority over a completed backlog.
                let _ = tx.send(Event::Finished {
                    code,
                    signal,
                    cancelled: stop.load(Ordering::Acquire),
                    elapsed: at.elapsed(),
                });
            })
            .map_err(|e| e.to_string())?;
        Ok(Self {
            events,
            cancel,
            finished,
            pid,
            input,
            worker: Some(worker),
        })
    }
    /// Queues a single line without waiting for pipe I/O or for space in the queue.
    pub fn send_line(&self, line: &str) -> Result<(), InputError> {
        if line.len() > MAX_INPUT_LINE || line.contains(['\r', '\n']) {
            return Err(InputError::InvalidLine);
        }
        if self.finished() || self.cancel.load(Ordering::Acquire) {
            return Err(InputError::Closed);
        }
        let input = self.input.as_ref().ok_or(InputError::Closed)?;
        let mut bytes = String::with_capacity(line.len() + 1);
        bytes.push_str(line);
        bytes.push('\n');
        input.try_send(bytes).map_err(|e| match e {
            mpsc::TrySendError::Full(_) => InputError::Full,
            mpsc::TrySendError::Disconnected(_) => InputError::Closed,
        })
    }
    /// Sends EOF after queued lines drain. It needs no space in the input queue.
    pub fn close_input(&mut self) {
        self.input = None;
    }
    /// Requests process-tree termination without blocking the UI.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Release);
        #[cfg(unix)]
        {
            let pid = self.pid.load(Ordering::Acquire);
            if pid != 0 {
                kill_tree(pid);
            }
        }
    }
    /// Whether the worker has reaped the child and drained output.
    pub fn finished(&self) -> bool {
        self.finished.load(Ordering::Acquire)
    }
}
impl Drop for Task {
    fn drop(&mut self) {
        self.cancel();
        // The worker owns/reaps its process group. UI teardown never joins pipe I/O.
        self.worker.take();
    }
}
fn run(
    exec: &Exec,
    tx: &crate::output::Sender,
    stop: &Arc<AtomicBool>,
    published_pid: &AtomicU32,
    incoming: Option<mpsc::Receiver<String>>,
) -> Result<(Option<i32>, Option<i32>), String> {
    let (env, warning) = crate::environment::launch_environment(exec, stop);
    if let Some(warning) = warning {
        let _ = send(tx, Event::Warning(warning), stop);
    }
    if stop.load(Ordering::Acquire) {
        return Ok((None, None));
    }
    let shell = exec.shell_cmd.clone().or_else(|| {
        exec.shell.then(|| {
            exec.cmd
                .iter()
                .map(|s| format!("'{}'", s.replace('\'', "'\\''")))
                .collect::<Vec<_>>()
                .join(" ")
        })
    });
    let mut command = if let Some(source) = shell {
        #[cfg(unix)]
        let mut command = Command::new("/bin/bash");
        #[cfg(not(unix))]
        let mut command = Command::new("cmd.exe");
        #[cfg(unix)]
        command.arg("-c");
        #[cfg(not(unix))]
        command.arg("/C");
        command.arg(source);
        command
    } else {
        let mut command = Command::new(exec.cmd.first().ok_or("empty build command")?);
        command.args(&exec.cmd[1..]);
        command
    };
    command
        .env_clear()
        .current_dir(&exec.working_dir)
        .envs(env)
        .stdin(if exec.interactive {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command.spawn().map_err(|e| e.to_string())?;
    let pid = child.id();
    published_pid.store(pid, Ordering::Release);
    #[cfg(unix)]
    let mut input =
        incoming
            .zip(child.stdin.take())
            .and_then(|(incoming, pipe)| match nonblocking(pipe.as_raw_fd()) {
                Ok(()) => Some(Input {
                    incoming,
                    pipe,
                    line: None,
                }),
                Err(_) => {
                    let _ = send(tx, Event::InputClosed, stop);
                    None
                }
            });
    #[cfg(not(unix))]
    drop(incoming);
    let exited = Arc::new(std::sync::OnceLock::new());
    let readers = [
        reader(
            child.stdout.take().expect("pipe"),
            Stream::Stdout,
            tx.clone(),
            exited.clone(),
            stop.clone(),
        ),
        reader(
            child.stderr.take().expect("pipe"),
            Stream::Stderr,
            tx.clone(),
            exited.clone(),
            stop.clone(),
        ),
    ];
    let status = loop {
        if stop.load(Ordering::Acquire) {
            #[cfg(unix)]
            drop(input.take());
            kill_tree(pid);
            let _ = child.kill();
            break child.wait();
        }
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) => {
                #[cfg(unix)]
                if let Some(writer) = &mut input {
                    match writer.flush_step(tx) {
                        Ok(true) => {}
                        Ok(false) => {
                            input = None;
                        }
                        Err(_) => {
                            input = None;
                            let _ = send(tx, Event::InputClosed, stop);
                        }
                    }
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(error) => {
                kill_tree(pid);
                let _ = child.wait();
                break Err(error);
            }
        }
    };
    #[cfg(unix)]
    drop(input);
    kill_tree(pid);
    published_pid.store(0, Ordering::Release);
    // Read for at most 500 ms after exit, even when a detached writer never stops.
    // The transport retains a bounded tail without waiting for UI delivery, so
    // backpressure cannot extend these joins. Cancel wakes within poll's timeout.
    let _ = exited.set(Instant::now());
    for reader in readers {
        let _ = reader.join();
    }
    status
        .map(|s| {
            #[cfg(unix)]
            {
                use std::os::unix::process::ExitStatusExt;
                (s.code(), s.signal())
            }
            #[cfg(not(unix))]
            {
                (s.code(), None)
            }
        })
        .map_err(|e| e.to_string())
}

#[cfg(unix)]
struct Input {
    incoming: mpsc::Receiver<String>,
    pipe: ChildStdin,
    line: Option<(String, usize)>,
}

#[cfg(unix)]
impl Input {
    // At most one nonblocking write per step; process exit and cancellation still get polled
    // even if the child or a descendant holds stdin open without reading it.
    fn flush_step(&mut self, tx: &crate::output::Sender) -> io::Result<bool> {
        if self.line.is_none() {
            match self.incoming.try_recv() {
                Ok(line) => {
                    let _ = tx.send(Event::Output {
                        stream: Stream::Input,
                        text: line.clone(),
                    });
                    self.line = Some((line, 0));
                }
                Err(mpsc::TryRecvError::Empty) => return Ok(true),
                Err(mpsc::TryRecvError::Disconnected) => return Ok(false),
            }
        }
        let (line, written) = self.line.as_mut().expect("pending input");
        match self.pipe.write(&line.as_bytes()[*written..]) {
            Ok(0) => Err(io::ErrorKind::WriteZero.into()),
            Ok(n) => {
                *written += n;
                if *written == line.len() {
                    self.line = None;
                }
                Ok(true)
            }
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                ) =>
            {
                Ok(true)
            }
            Err(e) => Err(e),
        }
    }
}

#[cfg(unix)]
fn nonblocking(fd: std::os::fd::RawFd) -> io::Result<()> {
    // SAFETY: the caller owns the live pipe end while fcntl reads and changes its flags.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    // SAFETY: this only changes status flags on the same live pipe.
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(all(test, unix))]
mod input_tests {
    use super::*;

    #[test]
    fn r33_child_that_never_reads_does_not_block_input_cancel_or_restart() {
        crate::test_login::install();
        let exec = |cmd: serde_json::Value| {
            crate::BuildSystem::from_value(
                "fixture",
                serde_json::json!({"cmd":cmd,"interactive":true}),
            )
            .unwrap()
            .resolve(&crate::Variables::default())
            .unwrap()
        };
        let mut task = Task::spawn(&exec(serde_json::json!(["/bin/sleep", "60"]))).unwrap();
        let line = "x".repeat(MAX_INPUT_LINE);
        let mut queued = 0;
        for _ in 0..64 {
            for _ in 0..16 {
                match task.send_line(&line) {
                    Ok(()) => queued += 1,
                    Err(InputError::Full) => break,
                    Err(e) => panic!("unexpected input status {e:?}"),
                }
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!((INPUT_QUEUE..64).contains(&queued), "queued {queued}");
        assert_eq!(task.send_line(&line), Err(InputError::Full));
        assert_eq!(
            task.send_line(&"x".repeat(MAX_INPUT_LINE + 1)),
            Err(InputError::InvalidLine)
        );
        task.close_input(); // EOF fits even when the queue is full.
        assert_eq!(task.send_line("after EOF"), Err(InputError::Closed));
        let mut next =
            Task::spawn_after(&exec(serde_json::json!(["/bin/cat"])), Some(task)).unwrap();
        next.send_line("replacement").unwrap();
        next.close_input();
        let by = Instant::now() + Duration::from_secs(2);
        let mut echoed = false;
        loop {
            match next
                .events
                .recv_timeout(by.saturating_duration_since(Instant::now()))
                .unwrap()
            {
                Event::Output {
                    stream: Stream::Stdout,
                    text,
                } => echoed |= text == "replacement\n",
                Event::Finished { code, .. } => {
                    assert_eq!(code, Some(0));
                    break;
                }
                _ => {}
            }
        }
        assert!(echoed);
    }
}
pub(crate) fn kill_tree(pid: u32) {
    #[cfg(unix)]
    {
        // SAFETY: the child created its own process group with PGID=PID. A negative
        // PID targets that group only; SIGKILL cannot run application signal handlers.
        unsafe {
            libc::kill(-(pid as i32), libc::SIGKILL);
        }
    }
    #[cfg(windows)]
    {
        let _ = Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .output();
    }
}
fn send(tx: &crate::output::Sender, event: Event, cancel: &AtomicBool) -> bool {
    !cancel.load(Ordering::Acquire) && tx.send(event)
}
#[cfg(unix)]
trait Pipe: Read + Send + std::os::fd::AsRawFd + 'static {}
#[cfg(unix)]
impl<T: Read + Send + std::os::fd::AsRawFd + 'static> Pipe for T {}
#[cfg(not(unix))]
trait Pipe: Read + Send + 'static {}
#[cfg(not(unix))]
impl<T: Read + Send + 'static> Pipe for T {}
fn reader(
    mut pipe: impl Pipe,
    stream: Stream,
    tx: crate::output::Sender,
    exited: Arc<std::sync::OnceLock<Instant>>,
    stop: Arc<AtomicBool>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        #[cfg(unix)]
        {
            if nonblocking(pipe.as_raw_fd()).is_err() {
                let _ = tx.send(Event::Output {
                    stream,
                    text: "\n[output truncated]\n[Build pipe error: setup failed]\n".into(),
                });
                return;
            }
        }
        let mut chunk = [0u8; 8192];
        let mut pending = Vec::new();

        loop {
            if stop.load(Ordering::Acquire) {
                break;
            }
            let remaining = exited
                .get()
                .map(|at| Duration::from_millis(500).saturating_sub(at.elapsed()));
            if remaining.is_some_and(|left| left.is_zero()) {
                let _ = tx.send(Event::Output {
                    stream,
                    text: "\n[output truncated]\n".into(),
                });
                break;
            }
            #[cfg(unix)]
            {
                let mut descriptor = libc::pollfd {
                    fd: pipe.as_raw_fd(),
                    events: libc::POLLIN,
                    revents: 0,
                };
                let timeout = remaining
                    .unwrap_or(Duration::from_millis(25))
                    .min(Duration::from_millis(25));
                // SAFETY: one live pipe descriptor, no pointer retained by poll.
                let ready =
                    unsafe { libc::poll(&mut descriptor, 1, timeout.as_millis().max(1) as i32) };
                if ready == 0 {
                    continue;
                }
                if ready < 0 {
                    if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                        continue;
                    }
                    let _ = tx.send(Event::Output {
                        stream,
                        text: "\n[output truncated]\n[Build pipe error: poll failed]\n".into(),
                    });
                    break;
                }
            }
            let n = match pipe.read(&mut chunk) {
                Ok(n) => n,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => continue,
                Err(e) => {
                    let _ = tx.send(Event::Output {
                        stream,
                        text: format!("\n[output truncated]\n[Build pipe error: {e}]\n"),
                    });
                    return;
                }
            };
            pending.extend_from_slice(&chunk[..n]);
            let valid = match std::str::from_utf8(&pending) {
                Ok(_) => pending.len(),
                Err(e) if e.error_len().is_none() && n > 0 => e.valid_up_to(),
                Err(_) => pending.len(),
            };
            if valid > 0 {
                let text = String::from_utf8_lossy(&pending[..valid]).into_owned();
                // Parsing and bounded retention happen here, independently of UI replay.
                if !tx.send(Event::Output { stream, text }) {
                    return;
                }
                pending.drain(..valid);
            }
            if n == 0 {
                return;
            }
        }
        if !pending.is_empty() {
            let _ = tx.send(Event::Output {
                stream,
                text: String::from_utf8_lossy(&pending).into_owned(),
            });
        }
    })
}
