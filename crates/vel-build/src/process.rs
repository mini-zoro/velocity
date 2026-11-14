//! Bounded UTF-8 streaming and Unix process-group cancellation.
use crate::Exec;
use std::{
    io::Read,
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender, TrySendError},
    },
    time::{Duration, Instant},
};
/// Origin of an output chunk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stream {
    /// Standard output.
    Stdout,
    /// Standard error.
    Stderr,
}
/// Incremental output or final process result. Completion follows both pipe readers.
#[derive(Debug)]
pub enum Event {
    /// Decoded text, including newline characters.
    Output {
        /// Origin.
        stream: Stream,
        /// Text with incomplete UTF-8 retained between reads.
        text: String,
    },
    /// Worker/pipe error.
    Error(String),
    /// Exit status and elapsed wall time.
    Finished {
        /// Process exit code (None after a signal).
        code: Option<i32>,
        /// Cancellation was requested.
        cancelled: bool,
        /// Spawn through pipe drain.
        elapsed: Duration,
    },
}
/// Asynchronous build handle. Drop cancels the complete process group.
#[derive(Debug)]
pub struct Task {
    /// Ordered events; consumers must poll to apply backpressure.
    pub events: Receiver<Event>,
    cancel: Arc<AtomicBool>,
    finished: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
}
impl Task {
    /// Spawns argv or shell_cmd with inherited stdin disabled and separate stdout/stderr pipes.
    pub fn spawn(exec: &Exec) -> Result<Self, String> {
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
            let mut c = Command::new("/bin/bash");
            #[cfg(not(unix))]
            let mut c = Command::new("cmd.exe");
            #[cfg(unix)]
            c.arg("-c");
            #[cfg(not(unix))]
            c.arg("/C");
            c.arg(source);
            c
        } else {
            let mut c = Command::new(exec.cmd.first().ok_or("empty build command")?);
            c.args(&exec.cmd[1..]);
            c
        };
        command
            .current_dir(&exec.working_dir)
            .envs(&exec.env)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let at = Instant::now();
        let mut child = command.spawn().map_err(|e| e.to_string())?;
        let pid = child.id();
        let stdout = child.stdout.take().expect("stdout pipe");
        let stderr = child.stderr.take().expect("stderr pipe");
        let cancel = Arc::new(AtomicBool::new(false));
        let finished = Arc::new(AtomicBool::new(false));
        let stop = cancel.clone();
        let done = finished.clone();
        let (tx, events) = mpsc::sync_channel(128);
        let worker = std::thread::Builder::new()
            .name("velocity-build".into())
            .spawn(move || {
                let readers = [
                    reader(stdout, Stream::Stdout, tx.clone(), stop.clone()),
                    reader(stderr, Stream::Stderr, tx.clone(), stop.clone()),
                ];
                let status = loop {
                    if stop.load(Ordering::Acquire) {
                        kill_tree(pid);
                        let _ = child.kill();
                        break child.wait();
                    }
                    match child.try_wait() {
                        Ok(Some(status)) => break Ok(status),
                        Ok(None) => std::thread::sleep(Duration::from_millis(5)),
                        Err(e) => break Err(e),
                    }
                };
                // Descendants inheriting pipes must not keep a completed build alive forever.
                kill_tree(pid);
                for reader in readers {
                    let _ = reader.join();
                }
                let code = match status {
                    Ok(s) => s.code(),
                    Err(e) => {
                        let _ = send(&tx, Event::Error(e.to_string()), &stop);
                        None
                    }
                };
                done.store(true, Ordering::Release);
                let _ = send(
                    &tx,
                    Event::Finished {
                        code,
                        cancelled: stop.load(Ordering::Acquire),
                        elapsed: at.elapsed(),
                    },
                    &stop,
                );
            })
            .map_err(|e| {
                kill_tree(pid);
                e.to_string()
            })?;
        Ok(Self {
            events,
            cancel,
            finished,
            worker: Some(worker),
        })
    }
    /// Requests process-tree termination without blocking the UI.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Release);
    }
    /// Whether the worker has reaped the child and drained output.
    pub fn finished(&self) -> bool {
        self.finished.load(Ordering::Acquire)
    }
}
impl Drop for Task {
    fn drop(&mut self) {
        self.cancel();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
fn kill_tree(pid: u32) {
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
fn send(tx: &SyncSender<Event>, mut event: Event, cancel: &AtomicBool) -> bool {
    loop {
        match tx.try_send(event) {
            Ok(()) => return true,
            Err(TrySendError::Disconnected(_)) => return false,
            Err(TrySendError::Full(e)) => {
                if cancel.load(Ordering::Acquire) {
                    return false;
                }
                event = e;
                std::thread::sleep(Duration::from_millis(2));
            }
        }
    }
}
fn reader(
    mut pipe: impl Read + Send + 'static,
    stream: Stream,
    tx: SyncSender<Event>,
    cancel: Arc<AtomicBool>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let mut chunk = [0u8; 8192];
        let mut pending = Vec::new();
        loop {
            let n = match pipe.read(&mut chunk) {
                Ok(n) => n,
                Err(e) => {
                    let _ = send(&tx, Event::Error(e.to_string()), &cancel);
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
                if !send(&tx, Event::Output { stream, text }, &cancel) {
                    return;
                }
                pending.drain(..valid);
            }
            if n == 0 {
                return;
            }
        }
    })
}
