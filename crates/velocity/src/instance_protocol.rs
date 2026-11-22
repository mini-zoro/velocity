//! The shared local IPC request and client-side wire protocol.
use crate::cli::{FileLocation, Options};
use serde::{Deserialize, Serialize};
use std::{
    io::{self, Read, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
#[cfg(unix)]
type ReplyStream = std::os::unix::net::UnixStream;
#[cfg(windows)]
type ReplyStream = crate::instance_pipe::Pipe;
pub(crate) const MAX_REQUEST: u64 = 64 * 1024 * 1024;
pub(crate) const TIMEOUT: Duration = Duration::from_secs(30);
pub(crate) const ENQUEUED: u8 = 1;
pub(crate) const BUSY: u8 = 4;

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
    pub stdin: Option<Stdin>,
    #[serde(default)]
    pub background: bool,
    #[serde(default)]
    pub stay: bool,
    #[serde(default)]
    pub command: Option<crate::cli::Named>,
    #[serde(default)]
    pub return_pid: Option<i32>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Stdin {
    Legacy(String),
    Bytes(Vec<u8>),
}
impl Stdin {
    pub fn as_bytes(&self) -> &[u8] {
        match self {
            Self::Legacy(s) => s.as_bytes(),
            Self::Bytes(bytes) => bytes,
        }
    }
}
#[cfg(test)]
impl From<&str> for Stdin {
    fn from(value: &str) -> Self {
        Self::Legacy(value.into())
    }
}
impl Request {
    pub fn new(options: &Options, stdin: Option<Stdin>) -> Result<Self, String> {
        #[cfg(unix)]
        let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
        #[cfg(unix)]
        let absolute = |p: &Path| {
            if p.is_absolute() {
                p.to_owned()
            } else {
                cwd.join(p)
            }
        };
        #[cfg(windows)]
        return Ok(Self {
            native_open: false,
            files: options
                .files
                .iter()
                .filter(|f| f.path != Path::new("-"))
                .map(|f| {
                    Ok(FileLocation {
                        path: std::path::absolute(&f.path).map_err(|e| e.to_string())?,
                        line: f.line,
                        column: f.column,
                    })
                })
                .collect::<Result<_, String>>()?,
            project: options
                .project
                .as_ref()
                .map(std::path::absolute)
                .transpose()
                .map_err(|e| e.to_string())?,
            new_window: options.new_window || options.launch_or_new_window,
            add: options.add_to_window,
            wait: options.wait || stdin.is_some(),
            stdin,
            background: options.background,
            stay: options.stay,
            command: options.command.clone(),
            return_pid: None,
        });
        #[cfg(unix)]
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
            new_window: options.new_window || options.launch_or_new_window,
            add: options.add_to_window,
            wait: options.wait || stdin.is_some(),
            stdin,
            background: options.background,
            stay: options.stay,
            command: options.command.clone(),
            return_pid: None,
        })
    }
}
/// A connected peer owns the request even if its acknowledgement never arrives.
pub enum Forward {
    NotRunning,
    Submitted(Result<(), String>),
}

pub(crate) fn submit(
    mut stream: ReplyStream,
    request: &Request,
    timeout: Duration,
    reopen: impl Fn() -> Option<ReplyStream>,
    accepted: impl FnOnce(),
    upgrade: Option<&str>,
) -> Result<(), String> {
    let mut acknowledged = false;
    let submit = (|| {
        let bytes = serde_json::to_vec(request).map_err(io::Error::other)?;
        if bytes.len() as u64 > MAX_REQUEST {
            return Err(io::Error::other(
                "request exceeds the local protocol size limit",
            ));
        }
        let until = Instant::now() + timeout;
        let busy = || io::Error::other("the editor is busy; request was not accepted");
        // Once the primary has explicitly answered BUSY, a later timeout or
        // failed reconnect still means "busy", not an opaque I/O error.
        let mut was_busy = false;
        loop {
            let attempt = (|| {
                let remaining = until
                    .saturating_duration_since(Instant::now())
                    .max(Duration::from_millis(1));
                stream.set_read_timeout(Some(remaining))?;
                stream.set_write_timeout(Some(remaining))?;
                stream.write_all(&(bytes.len() as u64).to_le_bytes())?;
                stream.write_all(&bytes)?;
                let mut ack = [0];
                stream.read_exact(&mut ack)?;
                Ok::<u8, io::Error>(ack[0])
            })();
            let ack = match attempt {
                Ok(ack) => ack,
                Err(_) if was_busy => return Err(busy()),
                Err(error) => return Err(error),
            };
            match ack {
                ENQUEUED => break,
                BUSY => {
                    was_busy = true;
                    let remaining = until.saturating_duration_since(Instant::now());
                    if remaining.is_zero() {
                        return Err(busy());
                    }
                    std::thread::sleep(remaining.min(Duration::from_millis(50)));
                    if Instant::now() >= until {
                        return Err(busy());
                    }
                    stream = reopen().ok_or_else(busy)?;
                }
                _ => return Err(io::Error::other("invalid enqueue acknowledgement")),
            }
        }
        acknowledged = true;
        accepted();
        if request.wait {
            let closed = || io::Error::other("connection to the editor closed");
            // Darwin can reject SO_RCVTIMEO after the peer exits. Its close
            // acknowledgement may still be buffered, so read before failing.
            if let Err(error) = stream.set_read_timeout(None)
                && !invalid_timeout(&error)
                && error.kind() != io::ErrorKind::NotConnected
            {
                return Err(error);
            }
            let read_error = |error: io::Error| match error.kind() {
                io::ErrorKind::UnexpectedEof
                | io::ErrorKind::ConnectionReset
                | io::ErrorKind::NotConnected => closed(),
                _ => error,
            };
            let mut ack = [0];
            stream.read_exact(&mut ack).map_err(read_error)?;
            if ack == [3] {
                let mut message = String::new();
                stream.take(4096).read_to_string(&mut message)?;
                return Err(io::Error::other(message));
            }
            if ack != [2] {
                return Err(io::Error::other("invalid close acknowledgement"));
            }
            if request.stdin.is_some() {
                let mut size = [0; 8];
                stream.read_exact(&mut size).map_err(read_error)?;
                let expected = u64::from_le_bytes(size);
                let mut stdout = io::stdout().lock();
                let copied = io::copy(&mut stream.take(expected), &mut stdout)?;
                if copied != expected {
                    return Err(io::Error::other("truncated edited stdin response"));
                }
                stdout.flush()?;
            }
            #[cfg(target_os = "macos")]
            if !request.background
                && !request.stay
                && let Some(pid) = request.return_pid
            {
                vel_platform::Platform::activate_pid(pid);
            }
        }
        Ok(())
    })();
    submit.map_err(|error| match (acknowledged, upgrade) {
        (true, _) => error.to_string(),
        (false, Some(upgrade)) => {
            format!("could not forward to the previous editor: {error}; {upgrade}")
        }
        (false, None) => format!("request sent to the editor: {error}"),
    })
}

fn invalid_timeout(error: &io::Error) -> bool {
    #[cfg(unix)]
    {
        error.raw_os_error() == Some(libc::EINVAL)
    }
    #[cfg(windows)]
    {
        error.kind() == io::ErrorKind::InvalidInput
    }
}
