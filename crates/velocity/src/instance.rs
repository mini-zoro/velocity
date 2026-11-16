//! Per-data-directory local-socket forwarding and close-driven CLI waiters.
use crate::{
    cli::{FileLocation, Options},
    workspace::Workspace,
};
use serde::{Deserialize, Serialize};
#[cfg(unix)]
use std::os::unix::{
    fs::{FileTypeExt, MetadataExt, PermissionsExt},
    net::{UnixListener, UnixStream},
};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Duration,
};
/// Serialized CLI request. All paths are made absolute at the sending process.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Request {
    /// Literal native paths: focus duplicate tabs without interpreting a CLI location.
    /// Older CLI/packaging clients omit this field and retain their existing behavior.
    #[serde(default)]
    pub native_open: bool,
    #[serde(skip)]
    pub(crate) queued_ack: bool,
    pub files: Vec<FileLocation>,
    pub project: Option<PathBuf>,
    pub new_window: bool,
    pub add: bool,
    pub wait: bool,
    pub stdin: Option<String>,
}
impl Request {
    pub fn new(options: &Options, stdin: Option<String>) -> Result<Self, String> {
        let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
        let absolute = |p: &Path| {
            if p.is_absolute() {
                p.to_owned()
            } else {
                cwd.join(p)
            }
        };
        Ok(Self {
            native_open: false,
            queued_ack: false,
            files: options
                .files
                .iter()
                .filter(|f| f.path != Path::new("-"))
                .map(|f| FileLocation {
                    path: absolute(&f.path),
                    line: f.line,
                    column: f.column,
                })
                .collect(),
            project: options.project.as_deref().map(absolute),
            new_window: options.new_window,
            add: options.add_to_window,
            wait: options.wait || stdin.is_some(),
            stdin,
        })
    }
}
#[cfg(unix)]
#[derive(Debug)]
pub struct Incoming {
    pub request: Request,
    pub stream: UnixStream,
}
#[cfg(unix)]
#[derive(Debug)]
pub struct Waiter {
    ids: std::collections::BTreeSet<u64>,
    stream: UnixStream,
    stdin_sheet: Option<u64>,
    stdin_output: bool,
    stdin_text: vel_text::Text,
    finished: bool,
}
#[cfg(unix)]
impl Waiter {
    pub fn acknowledge(
        incoming: Incoming,
        workspace: &mut Workspace,
        ids: std::collections::BTreeSet<u64>,
    ) -> Result<Option<Self>, String> {
        let Incoming {
            request,
            mut stream,
        } = incoming;
        // Native non-wait requests are acknowledged by the socket reader as soon as
        // they are queued. Waiting for UI application can block behind a modal dialog.
        if !request.queued_ack {
            stream.write_all(&[1]).map_err(|e| e.to_string())?;
        }
        if request.wait {
            let stdin_sheet = request.stdin.as_ref().and_then(|_| {
                ids.iter()
                    .rev()
                    .find(|id| workspace.stdout_sheets.contains(id))
                    .copied()
                    .or_else(|| ids.last().copied())
            });
            let stdin_text = stdin_sheet
                .and_then(|id| workspace.tabs.iter().find(|t| t.sheet == id))
                .map(|tab| tab.editor.text.clone())
                .unwrap_or_else(|| vel_text::Text::from(request.stdin.as_deref().unwrap_or("")));
            let mut waiter = Self {
                ids,
                stream,
                stdin_sheet,
                stdin_output: request.stdin.is_some(),
                stdin_text,
                finished: false,
            };
            if waiter.complete(workspace) {
                Ok(None)
            } else {
                Ok(Some(waiter))
            }
        } else {
            Ok(None)
        }
    }
    pub fn complete(&mut self, workspace: &mut Workspace) -> bool {
        if let Some(id) = self.stdin_sheet {
            if let Some(text) = workspace.closed_text.remove(&id) {
                self.stdin_text = text;
            } else if let Some(tab) = workspace.tabs.iter().find(|t| t.sheet == id) {
                self.stdin_text = tab.editor.text.clone();
            }
        }
        let done = self
            .ids
            .iter()
            .all(|id| !workspace.tabs.iter().any(|t| t.sheet == *id));
        if done {
            self.finish(workspace);
        }
        done
    }
    pub fn finish(&mut self, workspace: &mut Workspace) {
        if let Some(id) = self.stdin_sheet {
            if let Some(text) = workspace.closed_text.remove(&id) {
                self.stdin_text = text;
            } else if let Some(tab) = workspace.tabs.iter().find(|t| t.sheet == id) {
                self.stdin_text = tab.editor.text.clone();
            }
            workspace.stdout_sheets.remove(&id);
        }
        self.send_result();
    }
    fn send_result(&mut self) {
        if self.finished {
            return;
        }
        self.finished = true;
        let _ = self.stream.write_all(&[2]);
        if self.stdin_output {
            // A length-delimited UTF-8 payload follows the close acknowledgement.
            let _ = self
                .stream
                .write_all(&(self.stdin_text.len_bytes() as u64).to_le_bytes());
            for chunk in self.stdin_text.chunks() {
                let _ = self.stream.write_all(chunk.as_bytes());
            }
        }
    }
}
#[cfg(unix)]
impl Drop for Waiter {
    fn drop(&mut self) {
        self.send_result();
    }
}
/// Main-thread receiver; the listener owns no native handles.
#[cfg(unix)]
#[derive(Debug)]
pub struct Instance {
    rx: mpsc::Receiver<Incoming>,
    stop: Arc<AtomicBool>,
    path: PathBuf,
    thread: Option<std::thread::JoinHandle<()>>,
}
#[cfg(unix)]
pub enum Start {
    Forwarded,
    Primary(Instance),
}
#[cfg(unix)]
impl Instance {
    fn socket_path(data: Option<&Path>) -> PathBuf {
        use std::hash::{Hash, Hasher};
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        let data = data.map(|p| {
            if p.is_absolute() {
                p.to_owned()
            } else {
                std::env::current_dir().unwrap_or_default().join(p)
            }
        });
        data.hash(&mut hash);
        // SAFETY: geteuid has no arguments and does not mutate Rust state.
        let uid = unsafe { libc::geteuid() };
        std::env::temp_dir().join(format!("velocity-{uid}-{:016x}.sock", hash.finish()))
    }
    pub fn forward_existing(data: Option<&Path>, request: &Request) -> Result<bool, String> {
        Self::forward_with_ack(data, request, || {})
    }
    fn forward_with_ack(
        data: Option<&Path>,
        request: &Request,
        accepted: impl FnOnce(),
    ) -> Result<bool, String> {
        let Ok(mut stream) = UnixStream::connect(Self::socket_path(data)) else {
            return Ok(false);
        };
        let bytes = serde_json::to_vec(request).map_err(|e| e.to_string())?;
        stream
            .set_read_timeout(Some(if request.native_open {
                Duration::from_millis(500)
            } else {
                Duration::from_secs(30)
            }))
            .map_err(|e| e.to_string())?;
        stream
            .write_all(&(bytes.len() as u64).to_le_bytes())
            .and_then(|()| stream.write_all(&bytes))
            .map_err(|e| e.to_string())?;
        let mut ack = [0];
        if let Err(error) = stream.read_exact(&mut ack) {
            // Once a native request has been fully submitted it must not be retried as
            // a local open: the primary can still apply it after a busy UI recovers.
            if request.native_open
                && matches!(
                    error.kind(),
                    std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                )
            {
                return Ok(true);
            }
            return Err(error.to_string());
        }
        accepted();
        if request.wait {
            stream.set_read_timeout(None).map_err(|e| e.to_string())?;
            stream.read_exact(&mut ack).map_err(|e| e.to_string())?;
            if request.stdin.is_some() {
                let mut size = [0; 8];
                stream.read_exact(&mut size).map_err(|e| e.to_string())?;
                let mut remaining = u64::from_le_bytes(size);
                let mut stdout = std::io::stdout().lock();
                let copied = std::io::copy(&mut (&mut stream).take(remaining), &mut stdout)
                    .map_err(|e| e.to_string())?;
                remaining -= copied;
                if remaining != 0 {
                    return Err("truncated edited stdin response".into());
                }
                stdout.flush().map_err(|e| e.to_string())?;
            }
        }
        Ok(true)
    }
    pub fn start(data: Option<&Path>, request: &Request) -> Result<Start, String> {
        let path = Self::socket_path(data);
        if Self::forward_existing(data, request)? {
            return Ok(Start::Forwarded);
        }
        // SAFETY: geteuid is a side-effect-free POSIX identity query.
        let uid = unsafe { libc::geteuid() };
        if let Ok(metadata) = std::fs::symlink_metadata(&path) {
            if !metadata.file_type().is_socket() || metadata.uid() != uid {
                return Err(format!(
                    "instance socket is not owned by this user: {}",
                    path.display()
                ));
            }
            std::fs::remove_file(&path).map_err(|e| e.to_string())?;
        }
        let listener = UnixListener::bind(&path).map_err(|e| e.to_string())?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| e.to_string())?;
        listener.set_nonblocking(true).map_err(|e| e.to_string())?;
        let (tx, rx) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let done = stop.clone();
        let thread = std::thread::Builder::new()
            .name("velocity-cli".into())
            .spawn(move || {
                while !done.load(Ordering::Acquire) {
                    match listener.accept() {
                        Ok((mut stream, _)) => {
                            let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                            let mut size = [0; 8];
                            if stream.read_exact(&mut size).is_err() {
                                continue;
                            }
                            let size = u64::from_le_bytes(size);
                            if size > 64 * 1024 * 1024 {
                                continue;
                            }
                            let mut bytes = vec![0; size as usize];
                            if stream.read_exact(&mut bytes).is_err() {
                                continue;
                            }
                            let Ok(mut request) = serde_json::from_slice::<Request>(&bytes) else {
                                continue;
                            };
                            let _ = stream.set_read_timeout(None);
                            if request.native_open && !request.wait {
                                let _ = stream.write_all(&[1]);
                                request.queued_ack = true;
                            }
                            if tx.send(Incoming { request, stream }).is_err() {
                                break;
                            }
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(10))
                        }
                        Err(_) => break,
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Start::Primary(Self {
            rx,
            stop,
            path,
            thread: Some(thread),
        }))
    }
    pub fn drain(&self) -> Vec<Incoming> {
        self.rx.try_iter().collect()
    }
}
#[cfg(unix)]
impl Drop for Instance {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        let _ = std::fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn older_cli_requests_omit_native_marker_and_native_requests_roundtrip() {
        let mut value = serde_json::json!({"files":[],"project":null,"new_window":false,"add":false,"wait":false,"stdin":null});
        let legacy: super::Request = serde_json::from_value(value.clone()).unwrap();
        assert!(!legacy.native_open);
        value["native_open"] = serde_json::json!(true);
        let native: super::Request = serde_json::from_value(value).unwrap();
        assert!(native.native_open);
        let roundtrip: super::Request =
            serde_json::from_slice(&serde_json::to_vec(&native).unwrap()).unwrap();
        assert!(roundtrip.native_open);
    }
    use super::*;
    #[test]
    fn fresh_profile_socket_forwarding_waits_for_requested_sheet_close() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("does-not-exist/profile");
        let options = Options {
            wait: true,
            ..Options::default()
        };
        let request = Request::new(&options, None).unwrap();
        let Start::Primary(instance) = Instance::start(Some(&data), &request).unwrap() else {
            panic!("expected primary")
        };
        assert!(!data.exists());
        // Binding is synchronous in start(). Prepare the expensive workspace before
        // starting the client's acknowledgement timeout. Other workspace tests may
        // concurrently compile the full syntax corpus.
        let mut workspace = Workspace::new(vel_text::Text::new(), None, None, false).unwrap();
        let target = data.clone();
        let (accepted_tx, accepted_rx) = mpsc::channel();
        let (closed_tx, closed_rx) = mpsc::channel();
        let client = std::thread::spawn(move || {
            let result = Instance::forward_with_ack(Some(&target), &request, || {
                accepted_tx.send(()).unwrap();
            });
            closed_tx.send(result).unwrap();
        });
        // Channel delivery, not polling or a scheduler-speed assertion, establishes
        // readiness. This timeout is only a deadlock watchdog, not a performance gate.
        let watchdog = Duration::from_secs(120);
        let incoming = instance
            .rx
            .recv_timeout(watchdog)
            .expect("request delivered");
        assert!(incoming.request.stdin.is_none());
        let sheet = workspace.tabs[0].sheet;
        let mut waiter = Waiter::acknowledge(incoming, &mut workspace, [sheet].into())
            .unwrap()
            .unwrap();
        accepted_rx
            .recv_timeout(watchdog)
            .expect("accepted acknowledgement");
        assert!(matches!(
            closed_rx.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));
        assert!(!waiter.complete(&mut workspace));
        workspace.close(0).unwrap();
        assert!(waiter.complete(&mut workspace));
        assert!(
            closed_rx
                .recv_timeout(watchdog)
                .expect("close acknowledgement")
                .unwrap()
        );
        client.join().unwrap();
    }
}

#[cfg(all(test, unix))]
mod round15_tests {
    use super::*;
    use vel_text::Text;
    #[test]
    fn native_timeout_after_submission_does_not_report_failure_or_retry_locally() {
        let dir = tempfile::tempdir().unwrap();
        let path = Instance::socket_path(Some(dir.path()));
        let listener = UnixListener::bind(&path).unwrap();
        let (delivered, received) = mpsc::channel();
        let (release, resume) = mpsc::channel();
        let primary = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut size = [0; 8];
            stream.read_exact(&mut size).unwrap();
            let mut bytes = vec![0; u64::from_le_bytes(size) as usize];
            stream.read_exact(&mut bytes).unwrap();
            let request: Request = serde_json::from_slice(&bytes).unwrap();
            delivered.send(request).unwrap();
            resume.recv_timeout(Duration::from_secs(10)).unwrap();
            // The primary may still apply this queued request after the client returns.
        });
        let mut request = Request::new(&Options::default(), None).unwrap();
        request.native_open = true;
        assert!(Instance::forward_existing(Some(dir.path()), &request).unwrap());
        assert!(
            received
                .recv_timeout(Duration::from_secs(10))
                .unwrap()
                .native_open
        );
        release.send(()).unwrap();
        primary.join().unwrap();
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn stdin_implies_wait_and_returns_edited_utf8_when_the_sheet_closes() {
        let request = Request::new(&Options::default(), Some("original".into())).unwrap();
        assert!(request.wait);
        let (stream, mut client) = UnixStream::pair().unwrap();
        let mut w = Workspace::new(Text::new(), None, None, false).unwrap();
        w.editor_mut()
            .run("insert", &serde_json::json!({"characters":"original"}))
            .unwrap();
        let sheet = w.tabs[0].sheet;
        w.stdout_sheets.insert(sheet);
        let mut waiter = Waiter::acknowledge(Incoming { request, stream }, &mut w, [sheet].into())
            .unwrap()
            .unwrap();
        let mut ack = [0];
        client.read_exact(&mut ack).unwrap();
        assert_eq!(ack, [1]);
        w.editor_mut()
            .run("insert", &serde_json::json!({"characters":" 你好"}))
            .unwrap();
        w.close(0).unwrap();
        assert!(waiter.complete(&mut w));
        client.read_exact(&mut ack).unwrap();
        assert_eq!(ack, [2]);
        let mut size = [0; 8];
        client.read_exact(&mut size).unwrap();
        let mut text = vec![0; u64::from_le_bytes(size) as usize];
        client.read_exact(&mut text).unwrap();
        assert_eq!(String::from_utf8(text).unwrap(), "original 你好");
        assert!(w.closed_text.is_empty());
        assert!(w.stdout_sheets.is_empty());
        drop(waiter);
        client.set_nonblocking(true).unwrap();
        assert_eq!(client.read(&mut ack).unwrap(), 0);
    }
    #[test]
    fn window_close_returns_latest_stdin_and_native_ack_does_not_wait_for_ui() {
        let dir = tempfile::tempdir().unwrap();
        let request = Request::new(&Options::default(), None).unwrap();
        let Start::Primary(instance) = Instance::start(Some(dir.path()), &request).unwrap() else {
            panic!("primary")
        };
        let mut native = request;
        native.native_open = true;
        let data = dir.path().to_owned();
        let client = std::thread::spawn(move || Instance::forward_existing(Some(&data), &native));
        // No App/GUI has consumed a request, yet the listener already accepted it.
        assert!(client.join().unwrap().unwrap());
        let incoming = instance.rx.recv_timeout(Duration::from_secs(10)).unwrap();
        assert!(incoming.request.queued_ack);
        let mut w = Workspace::new(Text::new(), None, None, false).unwrap();
        assert!(
            Waiter::acknowledge(incoming, &mut w, Default::default())
                .unwrap()
                .is_none()
        );
        let (stream, mut client) = UnixStream::pair().unwrap();
        let request = Request::new(&Options::default(), Some(String::new())).unwrap();
        let sheet = w.tabs[0].sheet;
        w.stdout_sheets.insert(sheet);
        let mut waiter = Waiter::acknowledge(Incoming { request, stream }, &mut w, [sheet].into())
            .unwrap()
            .unwrap();
        w.editor_mut()
            .run("insert", &serde_json::json!({"characters":"latest"}))
            .unwrap();
        waiter.finish(&mut w);
        let mut data = vec![0; 2 + 8 + 6];
        client.read_exact(&mut data).unwrap();
        assert_eq!(&data[..2], &[1, 2]);
        assert_eq!(&data[10..], b"latest");
    }
}
