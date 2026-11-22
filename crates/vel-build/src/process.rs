//! Bounded UTF-8 streaming and process-tree cancellation.
use crate::Exec;
#[cfg(unix)]
use std::io;
#[cfg(not(windows))]
use std::process::{Command, Stdio};
#[cfg(unix)]
use std::sync::atomic::AtomicU32;
use std::{
    io::Read,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};
#[cfg(unix)]
use std::{io::Write, os::fd::AsRawFd, process::ChildStdin};
#[cfg(windows)]
use std::{io::Write, os::windows::io::AsRawHandle};
#[cfg(windows)]
mod windows;

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
    #[cfg(unix)]
    pid: Arc<AtomicU32>,
    #[cfg(windows)]
    job: Arc<windows::Job>,
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
        #[cfg(unix)]
        let pid = Arc::new(AtomicU32::new(0));
        #[cfg(unix)]
        let child_pid = pid.clone();
        #[cfg(windows)]
        let job = Arc::new(windows::Job::new().map_err(|e| e.to_string())?);
        #[cfg(windows)]
        let child_job = job.clone();
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
                let result = run(
                    &exec,
                    &tx,
                    &stop,
                    #[cfg(unix)]
                    &child_pid,
                    #[cfg(windows)]
                    &child_job,
                    incoming,
                );
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
            #[cfg(unix)]
            pid,
            #[cfg(windows)]
            job,
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
        let ending = if cfg!(windows) { "\r\n" } else { "\n" };
        let mut bytes = String::with_capacity(line.len() + ending.len());
        bytes.push_str(line);
        bytes.push_str(ending);
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
        #[cfg(windows)]
        self.job.kill();
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
    #[cfg(unix)] published_pid: &AtomicU32,
    #[cfg(windows)] job: &windows::Job,
    incoming: Option<mpsc::Receiver<String>>,
) -> Result<(Option<i32>, Option<i32>), String> {
    let (env, warning) = crate::environment::launch_environment(exec, stop);
    if let Some(warning) = warning {
        let _ = send(tx, Event::Warning(warning), stop);
    }
    if stop.load(Ordering::Acquire) {
        return Ok((None, None));
    }
    #[cfg(windows)]
    let mut child = windows::spawn(exec, &env, job).map_err(|e| e.to_string())?;
    #[cfg(not(windows))]
    let mut child = {
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
        command.spawn().map_err(|e| e.to_string())?
    };
    #[cfg(unix)]
    let pid = child.id();
    #[cfg(unix)]
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
    #[cfg(windows)]
    let input = incoming
        .zip(child.stdin.take())
        .map(|(incoming, mut pipe)| {
            let tx = tx.clone();
            let stop = stop.clone();
            let exited = Arc::new(AtomicBool::new(false));
            let done = exited.clone();
            let worker = std::thread::spawn(move || {
                while !stop.load(Ordering::Acquire) && !done.load(Ordering::Acquire) {
                    match incoming.recv_timeout(Duration::from_millis(25)) {
                        Ok(line) => {
                            let _ = send(
                                &tx,
                                Event::Output {
                                    stream: Stream::Input,
                                    text: line.clone(),
                                },
                                &stop,
                            );
                            if pipe.write_all(line.as_bytes()).is_err() {
                                let _ = send(&tx, Event::InputClosed, &stop);
                                break;
                            }
                        }
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    }
                }
            });
            (worker, exited)
        });
    #[cfg(not(any(unix, windows)))]
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
            #[cfg(unix)]
            kill_tree(pid);
            #[cfg(windows)]
            job.kill();
            #[cfg(not(windows))]
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
                #[cfg(unix)]
                kill_tree(pid);
                #[cfg(windows)]
                job.kill();
                let _ = child.wait();
                break Err(error);
            }
        }
    };
    #[cfg(unix)]
    drop(input);
    #[cfg(windows)]
    {
        job.kill();
        if let Some((worker, done)) = input {
            done.store(true, Ordering::Release);
            let _ = worker.join();
        }
    }
    #[cfg(unix)]
    kill_tree(pid);
    #[cfg(unix)]
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

    // Login capture alone can take two seconds before exec. These deadlines
    // detect hangs; event ordering tests restart independently of scheduling.
    const WATCHDOG: Duration = Duration::from_secs(30);

    #[track_caller]
    fn next_event(task: &Task, by: Instant, phase: &str) -> Event {
        task.events
            .recv_timeout(by.saturating_duration_since(Instant::now()))
            .unwrap_or_else(|error| panic!("{phase}: {error}"))
    }

    #[track_caller]
    fn expect_stdout(task: &Task, expected: &str, by: Instant, phase: &str) {
        let mut stdout = String::new();
        while stdout != expected {
            match next_event(task, by, phase) {
                Event::Output {
                    stream: Stream::Stdout,
                    text,
                } => {
                    stdout.push_str(&text);
                    assert!(expected.starts_with(&stdout), "{phase}: {stdout:?}");
                }
                event @ (Event::Error(_) | Event::Finished { .. } | Event::InputClosed) => {
                    panic!("{phase} before stdout: {event:?}");
                }
                _ => {}
            }
        }
    }

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
        let mut task = Task::spawn(&exec(serde_json::json!([
            "/bin/sh",
            "-c",
            "printf 'ready\\n'; exec /bin/sleep 60"
        ])))
        .unwrap();
        // A full channel before the child starts only tests the Rust queue.
        expect_stdout(
            &task,
            "ready\n",
            Instant::now() + WATCHDOG,
            "old child startup",
        );
        let line = "x".repeat(MAX_INPUT_LINE);
        let mut queued = 0;
        let fill_by = Instant::now() + WATCHDOG;
        let mut full_since = None;
        loop {
            assert!(
                Instant::now() < fill_by,
                "stdin did not saturate after {queued} lines"
            );
            match task.send_line(&line) {
                Ok(()) => {
                    queued += 1;
                    full_since = None;
                }
                Err(InputError::Full) => {
                    // Let the worker fill the pipe as well as the channel.
                    let since = full_since.get_or_insert_with(Instant::now);
                    if queued > INPUT_QUEUE && since.elapsed() >= Duration::from_millis(100) {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(e) => panic!("unexpected input status {e:?}"),
            }
        }
        assert_eq!(
            task.send_line(&"x".repeat(MAX_INPUT_LINE + 1)),
            Err(InputError::InvalidLine)
        );
        task.close_input(); // EOF fits even when the queue is full.
        assert_eq!(task.send_line("after EOF"), Err(InputError::Closed));

        // Keep the predecessor's join handle unfinished through the replacement's
        // output. A join in either spawn_after or the new worker breaks this order.
        let cancelled = task.cancel.clone();
        let old_worker = task.worker.take().unwrap();
        let (release_old, hold_old) = mpsc::channel::<()>();
        let (reaped, cleanup) = mpsc::channel();
        task.worker = Some(std::thread::spawn(move || {
            let _ = hold_old.recv();
            let _ = reaped.send(old_worker.join());
        }));
        let cat = exec(serde_json::json!(["/bin/cat"]));
        let (started, replacement) = mpsc::channel();
        // Bound even a synchronous join regression. Unwinding drops release_old.
        let by = Instant::now() + WATCHDOG;
        let starter = std::thread::spawn(move || {
            let _ = started.send(Task::spawn_after(&cat, Some(task)));
        });
        let mut next = replacement
            .recv_timeout(by.saturating_duration_since(Instant::now()))
            .expect("restart waited for the old worker")
            .unwrap();
        assert!(cancelled.load(Ordering::Acquire));
        next.send_line("replacement").unwrap();
        next.close_input();
        expect_stdout(&next, "replacement\n", by, "replacement build");
        drop(release_old);
        loop {
            match next_event(&next, by, "replacement EOF") {
                Event::Finished {
                    code,
                    signal,
                    cancelled,
                    ..
                } => {
                    assert_eq!((code, signal, cancelled), (Some(0), None, false));
                    break;
                }
                event @ (Event::Error(_) | Event::InputClosed) => {
                    panic!("replacement EOF: {event:?}")
                }
                _ => {}
            }
        }
        cleanup
            .recv_timeout(WATCHDOG)
            .expect("cancelled worker did not exit")
            .unwrap();
        starter.join().unwrap();
    }
}
#[cfg(unix)]
pub(crate) fn kill_tree(pid: u32) {
    // SAFETY: the child created its own process group with PGID=PID. A negative
    // PID targets that group only; SIGKILL cannot run application signal handlers.
    unsafe {
        libc::kill(-(pid as i32), libc::SIGKILL);
    }
}
fn send(tx: &crate::output::Sender, event: Event, cancel: &AtomicBool) -> bool {
    !cancel.load(Ordering::Acquire) && tx.send(event)
}
#[cfg(unix)]
trait Pipe: Read + Send + std::os::fd::AsRawFd + 'static {}
#[cfg(unix)]
impl<T: Read + Send + std::os::fd::AsRawFd + 'static> Pipe for T {}
#[cfg(windows)]
trait Pipe: Read + Send + AsRawHandle + 'static {}
#[cfg(windows)]
impl<T: Read + Send + AsRawHandle + 'static> Pipe for T {}
#[cfg(not(any(unix, windows)))]
trait Pipe: Read + Send + 'static {}
#[cfg(not(any(unix, windows)))]
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
            #[cfg(windows)]
            let read = match windows::available(pipe.as_raw_handle()) {
                Ok(Some(0)) => {
                    std::thread::sleep(Duration::from_millis(5));
                    continue;
                }
                Ok(Some(n)) => {
                    let count = n.min(chunk.len());
                    pipe.read(&mut chunk[..count])
                }
                Ok(None) => Ok(0),
                Err(e) => Err(e),
            };
            #[cfg(not(windows))]
            let read = pipe.read(&mut chunk);
            let n = match read {
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
