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
            wait: options.wait,
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
}
#[cfg(unix)]
impl Waiter {
    pub fn acknowledge(
        incoming: Incoming,
        workspace: &Workspace,
        ids: std::collections::BTreeSet<u64>,
    ) -> Result<Option<Self>, String> {
        let Incoming {
            request,
            mut stream,
        } = incoming;
        stream.write_all(&[1]).map_err(|e| e.to_string())?;
        if request.wait {
            let mut waiter = Self { ids, stream };
            if waiter.complete(workspace) {
                Ok(None)
            } else {
                Ok(Some(waiter))
            }
        } else {
            Ok(None)
        }
    }
    pub fn complete(&mut self, workspace: &Workspace) -> bool {
        let done = self
            .ids
            .iter()
            .all(|id| !workspace.tabs.iter().any(|t| t.sheet == *id));
        if done {
            let _ = self.stream.write_all(&[2]);
        }
        done
    }
}
#[cfg(unix)]
impl Drop for Waiter {
    fn drop(&mut self) {
        let _ = self.stream.write_all(&[2]);
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
            .set_read_timeout(Some(Duration::from_secs(30)))
            .map_err(|e| e.to_string())?;
        stream
            .write_all(&(bytes.len() as u64).to_le_bytes())
            .and_then(|()| stream.write_all(&bytes))
            .map_err(|e| e.to_string())?;
        let mut ack = [0];
        stream.read_exact(&mut ack).map_err(|e| e.to_string())?;
        accepted();
        if request.wait {
            stream.set_read_timeout(None).map_err(|e| e.to_string())?;
            stream.read_exact(&mut ack).map_err(|e| e.to_string())?;
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
                            let Ok(request) = serde_json::from_slice(&bytes) else {
                                continue;
                            };
                            let _ = stream.set_read_timeout(None);
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
        let request = Request::new(&options, Some("stdin 你好".into())).unwrap();
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
        assert_eq!(incoming.request.stdin.as_deref(), Some("stdin 你好"));
        let sheet = workspace.tabs[0].sheet;
        let mut waiter = Waiter::acknowledge(incoming, &workspace, [sheet].into())
            .unwrap()
            .unwrap();
        accepted_rx
            .recv_timeout(watchdog)
            .expect("accepted acknowledgement");
        assert!(matches!(
            closed_rx.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));
        assert!(!waiter.complete(&workspace));
        workspace.close(0).unwrap();
        assert!(waiter.complete(&workspace));
        assert!(
            closed_rx
                .recv_timeout(watchdog)
                .expect("close acknowledgement")
                .unwrap()
        );
        client.join().unwrap();
    }
}
