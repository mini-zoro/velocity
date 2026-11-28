//! Per-data-directory local-socket forwarding and close-driven CLI waiters.
#[cfg(all(unix, test))]
use crate::cli::Options;
#[cfg(unix)]
pub use crate::instance_protocol::Stdin;
#[cfg(unix)]
use crate::instance_protocol::submit;
use crate::instance_protocol::{BUSY, ENQUEUED, MAX_REQUEST, REJECTED, TIMEOUT};
pub use crate::instance_protocol::{Forward, Request};
use crate::workspace::Workspace;
#[cfg(unix)]
use std::os::{
    fd::{AsRawFd, FromRawFd},
    unix::{
        fs::{DirBuilderExt, FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt},
        net::{UnixListener, UnixStream},
        process::CommandExt,
    },
};
use std::{
    collections::VecDeque,
    io::Write,
    sync::{Condvar, Mutex, OnceLock, mpsc},
    time::{Duration, Instant},
};
#[cfg(unix)]
use std::{
    ffi::OsStr,
    fs::{self, File, OpenOptions},
    hash::{Hash, Hasher},
    io::{self, Read},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::SystemTime,
};
#[cfg(windows)]
mod win;
#[cfg(windows)]
use crate::instance_pipe::Pipe as ReplyStream;
#[cfg(windows)]
pub use win::{Claim, Instance, Start};
#[cfg(unix)]
type ReplyStream = UnixStream;

#[cfg(target_os = "macos")]
mod legacy_process;

#[cfg(unix)]
const PRIMARY_TIMEOUT: Duration = Duration::from_secs(3);
#[cfg(unix)]
const SOCKET_CHECK: Duration = Duration::from_millis(500);
#[cfg(unix)]
pub const INHERITED_LOCK: &str = "VELOCITY_INSTANCE_LOCK_FD";
#[derive(Debug)]
pub struct Incoming {
    pub request: Request,
    reply: Option<mpsc::Receiver<ReplyStream>>,
    delivery: Option<Delivery>,
}
const DELIVERY_WAIT: Duration = Duration::from_secs(5);
pub(crate) const DELIVERY_EXPIRED: &str =
    "no editor window accepted the request before the deadline";

#[derive(Debug)]
pub(crate) struct Delivery {
    deadline: Instant,
    stream: Option<ReplyStream>,
}

impl Delivery {
    pub(crate) fn expired(&self) -> bool {
        Instant::now() >= self.deadline
    }

    pub(crate) fn reject(mut self, error: &str) {
        if let Some(mut stream) = self.stream.take() {
            let mut reply = vec![REJECTED];
            // Stay within the client's bounded reply. Preserve UTF-8.
            reply.extend_from_slice(&error.as_bytes()[..error.floor_char_boundary(1024)]);
            let _ = stream.write_all(&reply);
        }
    }

    fn busy(mut self) {
        if let Some(mut stream) = self.stream.take() {
            let _ = stream.write_all(&[BUSY]);
        }
    }

    fn acknowledge(mut self, wait: bool) -> Option<mpsc::Receiver<ReplyStream>> {
        let mut stream = self.stream.take()?;
        stream.write_all(&[ENQUEUED]).ok()?;
        if wait {
            let (tx, rx) = mpsc::sync_channel(1);
            tx.send(stream).ok()?;
            Some(rx)
        } else {
            None
        }
    }
}
impl Drop for Delivery {
    fn drop(&mut self) {
        if let Some(mut stream) = self.stream.take() {
            let _ = stream.write_all(if self.expired() {
                b"\x03no editor window accepted the request before the deadline"
            } else {
                b"\x03the destination window closed or could not accept the request"
            });
        }
    }
}
impl Incoming {
    pub fn local(request: Request) -> Self {
        Self {
            request,
            reply: None,
            delivery: None,
        }
    }
    pub(crate) fn confirmed(&self) -> bool {
        self.delivery.is_some()
    }
    pub(crate) fn expired(&self) -> bool {
        self.delivery.as_ref().is_some_and(Delivery::expired)
    }
    pub(crate) fn busy(mut self) {
        if let Some(delivery) = self.delivery.take() {
            delivery.busy();
        }
    }
    pub(crate) fn reject(mut self, error: &str) {
        if let Some(delivery) = self.delivery.take() {
            delivery.reject(error);
        }
    }
    pub(crate) fn discard_payload(&mut self) {
        self.request.files = Vec::new();
        self.request.project = None;
        self.request.command = None;
        if self.request.stdin.is_some() {
            self.request.stdin = Some(crate::instance_protocol::Stdin::Bytes(Vec::new()));
        }
    }
    #[cfg(all(test, unix))]
    pub fn fixture(request: Request, mut stream: UnixStream) -> Self {
        let (tx, rx) = mpsc::sync_channel(1);
        stream.write_all(&[1]).unwrap();
        tx.send(stream).unwrap();
        let reply = request.wait.then_some(rx);
        Self {
            request,
            reply,
            delivery: None,
        }
    }
    #[cfg(all(test, unix))]
    pub(crate) fn confirmed_fixture(
        request: Request,
        stream: UnixStream,
        expires_in: Duration,
    ) -> Self {
        Self {
            request,
            reply: None,
            delivery: Some(Delivery {
                deadline: Instant::now() + expires_in,
                stream: Some(stream),
            }),
        }
    }
}

/// One expiring ingress queue per profile; expiry does not require a UI tick.
#[derive(Debug)]
struct Inbox {
    state: Mutex<InboxState>,
    changed: Condvar,
    capacity: usize,
}
#[derive(Debug, Default)]
struct InboxState {
    pending: VecDeque<(Incoming, u64)>,
    bytes: u64,
    closed: bool,
}
#[derive(Debug)]
struct Ingress(std::sync::Arc<Inbox>);
impl Ingress {
    fn start(capacity: usize) -> std::io::Result<Self> {
        let inbox = std::sync::Arc::new(Inbox {
            state: Mutex::new(InboxState::default()),
            changed: Condvar::new(),
            capacity,
        });
        let timer = inbox.clone();
        std::thread::Builder::new()
            .name("velocity-cli-expiry".into())
            .spawn(move || timer.expire())?;
        Ok(Self(inbox))
    }
    fn next(&self) -> Option<Incoming> {
        let mut state = self.0.state.lock().unwrap();
        let (incoming, bytes) = state.pending.pop_front()?;
        state.bytes -= bytes;
        Some(incoming)
    }
    #[cfg(test)]
    fn drain(&self) -> Vec<Incoming> {
        std::iter::from_fn(|| self.next()).collect()
    }
    #[cfg(all(test, unix))]
    fn recv_timeout(&self, timeout: Duration) -> Option<Incoming> {
        let state = self.0.state.lock().unwrap();
        let (mut state, _) = self
            .0
            .changed
            .wait_timeout_while(state, timeout, |state| {
                state.pending.is_empty() && !state.closed
            })
            .unwrap();
        let (incoming, bytes) = state.pending.pop_front()?;
        state.bytes -= bytes;
        Some(incoming)
    }
}
impl Drop for Ingress {
    fn drop(&mut self) {
        let pending = {
            let mut state = self.0.state.lock().unwrap();
            state.closed = true;
            std::mem::take(&mut state.pending)
        };
        self.0.changed.notify_all();
        drop(pending);
    }
}
#[derive(Debug, PartialEq, Eq)]
enum Admission {
    Accepted,
    Full,
    Closed,
}
impl Inbox {
    fn push(&self, incoming: Incoming, size: u64) -> Admission {
        let mut state = self.state.lock().unwrap();
        if state.closed {
            return Admission::Closed;
        }
        if state.pending.len() >= self.capacity || state.bytes + size > MAX_REQUEST {
            drop(state);
            incoming.busy();
            return Admission::Full;
        }
        state.bytes += size;
        state.pending.push_back((incoming, size));
        drop(state);
        self.changed.notify_all();
        Admission::Accepted
    }
    fn expire(&self) {
        let mut state = self.state.lock().unwrap();
        while !state.closed {
            if let Some(index) = state.pending.iter().position(|(item, _)| item.expired()) {
                let (item, size) = state.pending.remove(index).unwrap();
                state.bytes -= size;
                drop(state);
                item.reject(DELIVERY_EXPIRED);
                state = self.state.lock().unwrap();
            } else if let Some(deadline) = state
                .pending
                .iter()
                .filter_map(|(item, _)| item.delivery.as_ref().map(|reply| reply.deadline))
                .min()
            {
                let timeout = deadline.saturating_duration_since(Instant::now());
                (state, _) = self.changed.wait_timeout(state, timeout).unwrap();
            } else {
                state = self.changed.wait(state).unwrap();
            }
        }
    }
}

fn enqueue(tx: &Inbox, request: Request, size: u64, mut stream: ReplyStream) -> bool {
    if request.confirm_delivery {
        return tx.push(
            Incoming {
                request,
                reply: None,
                delivery: Some(Delivery {
                    deadline: Instant::now() + DELIVERY_WAIT,
                    stream: Some(stream),
                }),
            },
            size,
        ) != Admission::Closed;
    }
    let (reply, ready) = mpsc::sync_channel(1);
    let wait = request.wait;
    match tx.push(
        Incoming {
            request,
            reply: wait.then_some(ready),
            delivery: None,
        },
        size,
    ) {
        Admission::Accepted => {
            if stream.write_all(&[ENQUEUED]).is_ok() && wait {
                let _ = reply.send(stream);
            }
            true
        }
        Admission::Full => {
            let _ = stream.write_all(&[BUSY]);
            true
        }
        Admission::Closed => false,
    }
}
#[derive(Debug)]
pub struct Waiter {
    ids: std::collections::BTreeSet<u64>,
    reply: Option<mpsc::Receiver<ReplyStream>>,
    stdin_sheet: Option<u64>,
    stdin_output: bool,
    stdin_text: vel_text::Text,
    stdin_format: vel_fileio::Format,
}
impl Waiter {
    pub fn acknowledge(
        incoming: Incoming,
        workspace: &mut Workspace,
        ids: std::collections::BTreeSet<u64>,
    ) -> Result<Option<Self>, String> {
        let Incoming {
            request,
            reply,
            delivery,
        } = incoming;
        let reply =
            reply.or_else(|| delivery.and_then(|delivery| delivery.acknowledge(request.wait)));
        if let Some(reply) = reply {
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
                .unwrap_or_default();
            let stdin_format = stdin_sheet
                .and_then(|id| workspace.tabs.iter().find(|t| t.sheet == id))
                .map(|tab| tab.format.clone())
                .unwrap_or_default();
            let mut waiter = Self {
                ids,
                reply: Some(reply),
                stdin_sheet,
                stdin_output: request.stdin.is_some(),
                stdin_text,
                stdin_format,
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
        if let Some(reply) = self.reply.take() {
            let text = self
                .stdin_output
                .then(|| std::mem::take(&mut self.stdin_text));
            let format = self.stdin_format.clone();
            let _ = reply_worker().try_send(Box::new(move || {
                // The listener hands off the socket only after writing the enqueue ACK.
                if let Ok(mut stream) = reply.recv_timeout(TIMEOUT) {
                    let _ = stream.set_write_timeout(Some(Duration::from_millis(250)));
                    if let Some(text) = text {
                        let mut bytes = Vec::new();
                        if let Err(error) = vel_fileio::encode_to(&mut bytes, &text, &format) {
                            let _ = stream.write_all(&[3]);
                            let _ = stream.write_all(error.as_bytes());
                            return;
                        }
                        let _ = stream.write_all(&[2]);
                        let _ = stream.write_all(&(bytes.len() as u64).to_le_bytes());
                        let _ = stream.write_all(&bytes);
                    } else {
                        let _ = stream.write_all(&[2]);
                    }
                }
            }));
        }
    }
}

type ReplyJob = Box<dyn FnOnce() + Send>;
fn reply_worker() -> &'static mpsc::SyncSender<ReplyJob> {
    static WORKER: OnceLock<mpsc::SyncSender<ReplyJob>> = OnceLock::new();
    WORKER.get_or_init(|| {
        let (tx, rx) = mpsc::sync_channel::<ReplyJob>(32);
        std::thread::spawn(move || {
            for job in rx {
                job();
            }
        });
        tx
    })
}
pub fn finish_replies() {
    let (tx, rx) = mpsc::sync_channel(0);
    if reply_worker()
        .send(Box::new(move || {
            let _ = tx.send(());
        }))
        .is_ok()
    {
        let _ = rx.recv();
    }
}
#[cfg(unix)]
#[derive(Debug)]
pub struct Claim {
    path: PathBuf,
    lock_path: PathBuf,
    lock: File,
    starting: Option<Starting>,
}
pub enum Entry {
    Forwarded,
    Claimed(Claim),
}

#[cfg(unix)]
fn private_endpoint(path: PathBuf, uid: u32) -> io::Result<PathBuf> {
    if path.as_os_str().len() > 103 {
        return Err(io::Error::other(
            "instance socket name exceeds Darwin sun_path",
        ));
    }
    let base = path.parent().expect("socket parent");
    match fs::DirBuilder::new().mode(0o700).create(base) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e),
    }
    let meta = base.symlink_metadata()?;
    if !meta.is_dir() || meta.uid() != uid || meta.mode() & 0o777 != 0o700 {
        return Err(io::Error::other("instance socket directory is not private"));
    }
    Ok(path)
}

#[cfg(target_os = "macos")]
fn user_temp() -> io::Result<PathBuf> {
    use std::os::unix::ffi::OsStringExt;
    // https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man3/confstr.3.html
    // SAFETY: the first call measures the string including its NUL, the second
    // receives a writable buffer of exactly that size.
    unsafe {
        let name = libc::_CS_DARWIN_USER_TEMP_DIR;
        let size = libc::confstr(name, std::ptr::null_mut(), 0);
        if size <= 1 {
            return Err(io::Error::last_os_error());
        }
        let mut bytes = vec![0; size];
        if libc::confstr(name, bytes.as_mut_ptr().cast(), bytes.len()) != size {
            return Err(io::Error::other(
                "could not read Darwin user temp directory",
            ));
        }
        bytes.pop();
        Ok(PathBuf::from(std::ffi::OsString::from_vec(bytes)))
    }
}
#[cfg(all(unix, not(target_os = "macos")))]
fn user_temp() -> io::Result<PathBuf> {
    Ok(std::env::temp_dir())
}

#[cfg(unix)]
fn choose_endpoint(
    path: PathBuf,
    uid: u32,
    fallback: impl FnOnce() -> io::Result<PathBuf>,
) -> io::Result<PathBuf> {
    let name = path.file_name().expect("socket filename").to_owned();
    match private_endpoint(path, uid) {
        Ok(path) => Ok(path),
        Err(first) => fallback()
            .and_then(|directory| {
                private_endpoint(directory.join(format!("velocity-{uid}")).join(name), uid)
            })
            .map_err(|second| {
                io::Error::other(format!(
                    "no private instance socket directory ({first}; {second})"
                ))
            }),
    }
}

// 2026-11-19: retain this exact pre-flock lookup for at least two releases.
// 5497418 hashed Option<PathBuf> without canonicalizing away symlinks or `..`.
#[cfg(unix)]
fn legacy_path(data: Option<&Path>, uid: u32, tmp: &Path) -> PathBuf {
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    let data = data.map(|p| {
        if p.is_absolute() {
            p.to_owned()
        } else {
            std::env::current_dir().unwrap_or_default().join(p)
        }
    });
    data.hash(&mut hash);
    tmp.join(format!("velocity-{uid}-{:016x}.sock", hash.finish()))
}

#[cfg(unix)]
struct Endpoints {
    socket: PathBuf,
    lock: PathBuf,
    legacy: Vec<PathBuf>,
}

#[cfg(unix)]
impl Endpoints {
    fn new(data: Option<&Path>, isolated: Option<&Path>) -> io::Result<Self> {
        use crate::instance_path::{canonical_profile, socket_path};
        // SAFETY: geteuid is a side-effect-free identity query.
        let uid = unsafe { libc::geteuid() };
        let default = socket_path(data, uid);
        let os_temp = user_temp().ok();
        let socket = if let Some(directory) = isolated {
            private_endpoint(
                directory.join(default.file_name().expect("socket filename")),
                uid,
            )?
        } else {
            choose_endpoint(default, uid, || {
                os_temp
                    .clone()
                    .ok_or_else(|| io::Error::other("no user temp directory"))
            })?
        };
        let canonical = data.map(canonical_profile);
        // Lock the profile directory itself. The tmp cleaner cannot unlink
        // it, and acquiring an existing profile needs no writable disk space.
        let lock = canonical.as_deref().map_or_else(
            || socket.parent().expect("socket parent").to_owned(), // No session without a profile.
            Path::to_owned,
        );
        let mut legacy = Vec::new();
        let temps = [
            Some(std::env::temp_dir()),
            if isolated.is_none() { os_temp } else { None },
        ];
        for temp in temps.into_iter().flatten() {
            for spelling in [data, canonical.as_deref()] {
                let path = legacy_path(spelling, uid, &temp);
                if !legacy.contains(&path) {
                    legacy.push(path);
                }
            }
        }
        Ok(Self {
            socket,
            lock,
            legacy,
        })
    }

    fn for_profile(data: Option<&Path>) -> io::Result<Self> {
        // An alternate socket location cannot bypass the per-profile lock.
        // Keep the default profile independent of the caller's environment.
        let isolated = data.and_then(|path| {
            let default = std::env::var_os("HOME")
                .map(|home| PathBuf::from(home).join("Library/Application Support/Velocity"));
            (default.as_deref() != Some(path))
                .then(|| std::env::var_os("VELOCITY_INSTANCE_SOCKET_DIR"))
                .flatten()
        });
        Self::new(data, isolated.as_deref().map(Path::new))
    }

    fn forward_legacy(&self, request: &Request) -> Result<bool, String> {
        const UPGRADE: &str = "quit the running Velocity first";
        for path in &self.legacy {
            // SAFETY: geteuid is a side-effect-free identity query.
            let uid = unsafe { libc::geteuid() };
            let Some(stream) = legacy_connection(path, uid)? else {
                continue;
            };
            let mut request = request.clone();
            if request.command.is_some() {
                return Err(format!(
                    "the previous editor does not support --command; {UPGRADE}"
                ));
            }
            if let Some(Stdin::Bytes(bytes)) = request
                .stdin
                .take_if(|stdin| matches!(stdin, Stdin::Bytes(_)))
            {
                request.stdin = Some(Stdin::Legacy(String::from_utf8(bytes).map_err(|_| {
                    format!("the previous editor only accepts UTF-8 from stdin; {UPGRADE}")
                })?));
            }
            return submit(
                stream,
                &request,
                TIMEOUT,
                || UnixStream::connect(path).ok(),
                || {},
                Some(UPGRADE),
            )
            .map(|()| true);
        }
        Ok(false)
    }
}

#[cfg(unix)]
fn socket_identity(path: &Path, uid: u32) -> io::Result<(u64, u64)> {
    let meta = path.symlink_metadata()?;
    if meta.file_type().is_socket() && meta.uid() == uid {
        Ok((meta.dev(), meta.ino()))
    } else {
        Err(io::Error::other(
            "instance socket is not owned by this user",
        ))
    }
}

#[cfg(unix)]
fn legacy_connection(path: &Path, uid: u32) -> Result<Option<UnixStream>, String> {
    // An absent endpoint needs no retry: enter checks again after taking the lock.
    let Ok(identity) = socket_identity(path, uid) else {
        return Ok(None);
    };
    let refused = match UnixStream::connect(path) {
        Ok(stream) => return Ok(Some(stream)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => false,
        Err(error) if error.kind() == io::ErrorKind::ConnectionRefused => true,
        Err(_) => return Ok(None),
    };
    std::thread::sleep(Duration::from_millis(100));
    let Ok(second) = socket_identity(path, uid) else {
        return Ok(None);
    };
    match UnixStream::connect(path) {
        Ok(stream) => Ok(Some(stream)),
        Err(error) => {
            if error.kind() == io::ErrorKind::ConnectionRefused {
                let current = socket_identity(path, uid).ok();
                // The previous release does not flock. Refusal by a
                // replacement inode says nothing about its liveness.
                if let Some(current) = current {
                    if !refused || second != identity || current != identity {
                        return Err("Velocity is running but not responding; quit it first.".into());
                    }
                    #[cfg(target_os = "macos")]
                    if legacy_process::has_listener(path, uid, identity) {
                        return Err("Velocity is running but not responding; quit it first.".into());
                    }
                    if socket_identity(path, uid).ok() == Some(identity)
                        && fs::remove_file(path).is_ok()
                    {
                        eprintln!("velocity: removed stale legacy socket {}", path.display());
                    }
                }
            }
            Ok(None)
        }
    }
}

#[cfg(unix)]
#[derive(Debug)]
struct Starting {
    path: PathBuf,
    identity: (u64, u64),
}

#[cfg(unix)]
impl Starting {
    fn path(socket: &Path) -> PathBuf {
        socket.with_extension("starting")
    }

    fn mark(socket: &Path) -> io::Result<Self> {
        let path = Self::path(socket);
        // The previous owner no longer holds the profile lock.
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        let meta = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)?
            .metadata()?;
        Ok(Self {
            path,
            identity: (meta.dev(), meta.ino()),
        })
    }

    fn existing(socket: &Path) -> Option<Self> {
        let path = Self::path(socket);
        let meta = path.symlink_metadata().ok()?;
        meta.is_file().then(|| Self {
            path,
            identity: (meta.dev(), meta.ino()),
        })
    }

    fn fresh(socket: &Path, max_age: Duration) -> bool {
        Self::path(socket)
            .symlink_metadata()
            .ok()
            .filter(|meta| meta.is_file())
            .and_then(|meta| SystemTime::now().duration_since(meta.modified().ok()?).ok())
            .is_some_and(|age| age < max_age)
    }

    fn remove(self) {
        if self
            .path
            .symlink_metadata()
            .is_ok_and(|m| (m.dev(), m.ino()) == self.identity)
        {
            let _ = fs::remove_file(self.path);
        }
    }
}

#[cfg(unix)]
impl Claim {
    pub fn path(&self) -> &Path {
        &self.path
    }
    fn try_lock(endpoints: &Endpoints) -> io::Result<Option<Self>> {
        fs::DirBuilder::new()
            .mode(0o700)
            .recursive(true)
            .create(&endpoints.lock)?;
        let lock = Self::open_lock(&endpoints.lock)?;
        // Apple flock(2): the lock follows the open file description, including
        // across fork/exec. Never unlink the lock file or replace its inode.
        // https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/flock.2.html
        // SAFETY: lock owns a live file descriptor.
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            let claim = Self {
                path: endpoints.socket.clone(),
                lock_path: endpoints.lock.clone(),
                lock,
                starting: None,
            };
            claim.check_inode()?;
            return Ok(Some(claim));
        }
        let error = io::Error::last_os_error();
        if error.kind() == io::ErrorKind::WouldBlock {
            Ok(None)
        } else {
            Err(error)
        }
    }
    fn open_lock(path: &Path) -> io::Result<File> {
        OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY)
            .open(path)
    }
    pub fn pass_to(&self, command: &mut std::process::Command) {
        let fd = self.lock.as_raw_fd();
        command.env(INHERITED_LOCK, fd.to_string());
        // SAFETY: launch holds Claim across spawn; fcntl is async-signal-safe.
        unsafe {
            command.pre_exec(move || {
                if libc::fcntl(fd, libc::F_SETFD, 0) == -1 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    pub fn inherited(data: Option<&Path>, marker: Option<&OsStr>) -> Option<Self> {
        let fd: i32 = marker?.to_str()?.parse().ok().filter(|fd| *fd > 2)?;
        let endpoints = Endpoints::for_profile(data).ok()?;
        // SAFETY: fcntl validates the inherited descriptor and creates a new
        // owned copy of the same open file description, marked close-on-exec.
        let copy = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 3) };
        if copy < 0 {
            return None;
        }
        // SAFETY: copy is a newly owned descriptor.
        let lock = unsafe { File::from_raw_fd(copy) };
        let mut claim = Self {
            path: endpoints.socket,
            lock_path: endpoints.lock,
            lock,
            starting: None,
        };
        claim.check_inode().ok()?;
        // The launcher already holds the lock. Don't treat an arbitrary fd to
        // the profile (for example a shell's saved cwd) as a valid handoff.
        let probe = Self::open_lock(&claim.lock_path).ok()?;
        // SAFETY: probe is an independent, live descriptor.
        if unsafe { libc::flock(probe.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0
            || io::Error::last_os_error().kind() != io::ErrorKind::WouldBlock
        {
            return None;
        }
        // SAFETY: lock is live; fd was passed exclusively to this program by
        // the launcher. Closing the inherited copy leaves lock held by copy.
        unsafe {
            if libc::flock(copy, libc::LOCK_EX | libc::LOCK_NB) < 0 {
                return None;
            }
            libc::close(fd);
        }
        claim.starting = Starting::existing(&claim.path);
        Some(claim)
    }

    fn check_inode(&self) -> io::Result<()> {
        let want = self.lock_path.symlink_metadata()?;
        let got = self.lock.metadata()?;
        // SAFETY: geteuid is a side-effect-free identity query.
        let uid = unsafe { libc::geteuid() };
        if !got.is_dir() || got.uid() != uid || (want.dev(), want.ino()) != (got.dev(), got.ino()) {
            return Err(io::Error::other(
                "instance lock does not match this profile",
            ));
        }
        Ok(())
    }

    /// After spawning successfully, the inheriting child owns marker cleanup.
    pub fn handoff(mut self) {
        self.starting = None;
    }

    fn clear_starting(&mut self) {
        if let Some(marker) = self.starting.take() {
            marker.remove();
        }
    }

    pub fn listen(mut self) -> io::Result<Instance> {
        self.check_inode()?;
        let lock = self.lock.try_clone()?;
        // Only the process that holds the lock may replace or remove a socket.
        match self.path.symlink_metadata() {
            Ok(meta) => {
                // SAFETY: side-effect-free identity query.
                let uid = unsafe { libc::geteuid() };
                if !meta.file_type().is_socket() || meta.uid() != uid {
                    return Err(io::Error::other(
                        "instance socket is not owned by this user",
                    ));
                }
                fs::remove_file(&self.path)?;
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        let listener = BoundSocket::bind(&self.path)?;
        let rx = Ingress::start(16)?;
        let tx = rx.0.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let cancel = stop.clone();
        let (wake, wake_read) = UnixStream::pair()?;
        self.clear_starting();
        std::thread::Builder::new()
            .name("velocity-cli".into())
            .spawn(move || {
                // Keep Claim (and the lock) until cleanup is done.
                let _claim = self;
                serve(listener, wake_read, &tx, &cancel);
            })?;
        Ok(Instance {
            rx,
            stop,
            _wake: wake,
            _lock: lock,
        })
    }
}

#[cfg(unix)]
impl Drop for Claim {
    fn drop(&mut self) {
        self.clear_starting();
    }
}

#[cfg(unix)]
struct BoundSocket {
    listener: UnixListener,
    path: PathBuf,
    uid: u32,
    identity: Option<(u64, u64)>,
}

#[cfg(unix)]
impl BoundSocket {
    fn bind(path: &Path) -> io::Result<Self> {
        // SAFETY: side-effect-free identity query.
        let uid = unsafe { libc::geteuid() };
        let path = private_endpoint(path.to_owned(), uid)?;
        let listener = UnixListener::bind(&path)?;
        let identity = socket_identity(&path, uid)?;
        let socket = Self {
            listener,
            path,
            uid,
            identity: Some(identity),
        };
        fs::set_permissions(&socket.path, fs::Permissions::from_mode(0o600))?;
        Ok(socket)
    }
}

#[cfg(unix)]
impl Drop for BoundSocket {
    fn drop(&mut self) {
        if self.identity.is_some() && socket_identity(&self.path, self.uid).ok() == self.identity {
            let _ = fs::remove_file(&self.path);
        }
    }
}

#[cfg(unix)]
fn serve(mut socket: BoundSocket, wake: UnixStream, tx: &Inbox, stop: &AtomicBool) {
    let mut fds = [
        libc::pollfd {
            fd: socket.listener.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        },
        libc::pollfd {
            fd: wake.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        },
    ];
    let mut next_check = Instant::now() + SOCKET_CHECK;
    while !stop.load(Ordering::Acquire) {
        let now = Instant::now();
        if now >= next_check {
            next_check = now + SOCKET_CHECK;
            // Keep accepting connections on the old fd until the missing path
            // can be rebound. Never overwrite an unexpected replacement.
            if let Err(error) = socket_identity(&socket.path, socket.uid)
                && error.kind() == io::ErrorKind::NotFound
                && let Ok(rebound) = BoundSocket::bind(&socket.path)
            {
                fds[0].fd = rebound.listener.as_raw_fd();
                // Do not unlink the new socket even if its inode was reused.
                socket.identity = None;
                socket = rebound;
            }
        }
        let millis = next_check.saturating_duration_since(now).as_millis() as i32 + 1;
        // SAFETY: fds has two initialized descriptors, alive for the call.
        if unsafe { libc::poll(fds.as_mut_ptr(), 2, millis) } < 0 {
            if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                continue;
            }
            break;
        }
        if fds[1].revents != 0 {
            break;
        }
        if fds[0].revents & libc::POLLIN == 0 {
            continue;
        }
        let Ok((mut stream, _)) = socket.listener.accept() else {
            continue;
        };
        let _ = stream.set_read_timeout(Some(Duration::from_millis(100)));
        let _ = stream.set_write_timeout(Some(Duration::from_millis(100)));
        let until = Instant::now() + Duration::from_secs(5);
        let mut size = [0; 8];
        if read_request(&mut stream, &mut size, until, stop).is_err() {
            continue;
        }
        let size = u64::from_le_bytes(size);
        if size > MAX_REQUEST {
            continue;
        }
        let mut bytes = vec![0; size as usize];
        if read_request(&mut stream, &mut bytes, until, stop).is_err() {
            continue;
        }
        let Ok(request) = serde_json::from_slice::<Request>(&bytes) else {
            continue;
        };
        // The listener drains into this bounded queue even during a native
        // modal. The App only applies those requests on its regular event path.
        if !enqueue(tx, request, size, stream) {
            break;
        }
    }
}

#[cfg(unix)]
fn read_request(
    stream: &mut UnixStream,
    mut buf: &mut [u8],
    until: Instant,
    stop: &AtomicBool,
) -> io::Result<()> {
    while !buf.is_empty() {
        if stop.load(Ordering::Acquire) || Instant::now() >= until {
            return Err(io::ErrorKind::TimedOut.into());
        }
        match stream.read(buf) {
            Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
            Ok(n) => buf = &mut buf[n..],
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::Interrupted
                        | io::ErrorKind::WouldBlock
                        | io::ErrorKind::TimedOut
                ) => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// Main-thread receiver; the listener owns no native handles.
#[cfg(unix)]
#[derive(Debug)]
pub struct Instance {
    rx: Ingress,
    stop: Arc<AtomicBool>,
    // Releasing this descriptor wakes the listener without a main-thread join.
    _wake: UnixStream,
    // A failed listener must not unlock a still-live GUI.
    _lock: File,
}
#[cfg(unix)]
pub enum Start {
    Forwarded,
    Primary(Instance),
}

#[cfg(unix)]
impl Instance {
    pub fn forward(path: &Path, request: &Request) -> Result<Forward, String> {
        Self::forward_with_ack(path, request, TIMEOUT, || {})
    }

    fn forward_with_ack(
        path: &Path,
        request: &Request,
        timeout: Duration,
        accepted: impl FnOnce(),
    ) -> Result<Forward, String> {
        let open = || match UnixStream::connect(path) {
            Ok(stream) => Ok(Some(stream)),
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
                ) =>
            {
                Ok(None)
            }
            Err(e) => Err(format!("cannot connect to the editor: {e}")),
        };
        let Some(stream) = open()? else {
            return Ok(Forward::NotRunning);
        };
        Ok(Forward::Submitted(submit(
            stream,
            request,
            timeout,
            || open().ok().flatten(),
            accepted,
            None,
        )))
    }

    pub fn enter(data: Option<&Path>, request: &Request) -> Result<Entry, String> {
        let endpoints = Endpoints::for_profile(data).map_err(|e| e.to_string())?;
        Self::enter_at(&endpoints, request)
    }

    fn enter_at(endpoints: &Endpoints, request: &Request) -> Result<Entry, String> {
        Self::enter_until(endpoints, request, PRIMARY_TIMEOUT, TIMEOUT)
    }

    fn enter_until(
        endpoints: &Endpoints,
        request: &Request,
        ready_timeout: Duration,
        starting_timeout: Duration,
    ) -> Result<Entry, String> {
        let at = Instant::now();
        loop {
            match Self::forward(&endpoints.socket, request)? {
                Forward::Submitted(result) => return result.map(|()| Entry::Forwarded),
                Forward::NotRunning => {}
            }
            if endpoints.forward_legacy(request)? {
                return Ok(Entry::Forwarded);
            }
            if let Some(mut claim) = Claim::try_lock(endpoints).map_err(|e| e.to_string())? {
                // The previous owner can have booted and shut down between
                // attempts. Only a lock owner can make the startup decision.
                if endpoints.forward_legacy(request)? {
                    return Ok(Entry::Forwarded);
                }
                // Best effort: a full or read-only filesystem must not
                // prevent an editor from starting. A killed starter leaves a
                // marker that expires after the maximum cold-start delay.
                claim.starting = Starting::mark(&claim.path).ok();
                return Ok(Entry::Claimed(claim));
            }
            let wait = at.elapsed();
            if wait >= ready_timeout
                && (wait >= starting_timeout
                    || !Starting::fresh(&endpoints.socket, starting_timeout))
            {
                // Binding removes the marker. Check once more in case the
                // socket appeared between this turn's connect and stat.
                if let Forward::Submitted(result) = Self::forward(&endpoints.socket, request)? {
                    return result.map(|()| Entry::Forwarded);
                }
                return Err("the running editor is not responding".into());
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    pub fn start(data: Option<&Path>, request: &Request) -> Result<Start, String> {
        Self::start_entry(Self::enter(data, request)?)
    }
    #[cfg(test)]
    pub fn start_fixture(root: &Path, request: &Request) -> Result<Start, String> {
        Self::start_entry(Self::enter_at(&fixture_endpoints(root), request)?)
    }
    fn start_entry(entry: Entry) -> Result<Start, String> {
        match entry {
            Entry::Forwarded => Ok(Start::Forwarded),
            Entry::Claimed(claim) => claim
                .listen()
                .map(Start::Primary)
                .map_err(|e| e.to_string()),
        }
    }
    #[cfg(test)]
    pub fn drain(&self) -> Vec<Incoming> {
        self.rx.drain()
    }
    pub fn next(&self) -> Option<Incoming> {
        self.rx.next()
    }
}
#[cfg(unix)]
impl Drop for Instance {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}

#[cfg(all(test, unix))]
fn fixture_endpoints(root: &Path) -> Endpoints {
    let mut endpoints =
        Endpoints::new(Some(&root.join("profile")), Some(&root.join("ipc"))).unwrap();
    endpoints.legacy.clear();
    endpoints
}

#[cfg(all(test, unix))]
mod tests {
    #[test]
    fn stable_endpoint_ignores_shell_tmpdir_and_canonicalizes_a_fresh_profile() {
        use crate::instance_path::socket_path;
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(
            socket_path(None, 501),
            Path::new("/tmp/velocity-501/cbf29ce484222325.sock")
        );
        let alias = tmp.path().join("alias");
        std::os::unix::fs::symlink(tmp.path(), &alias).unwrap();
        let suffix = "long".repeat(58);
        let via_link = socket_path(Some(&alias.join(&suffix)), 501);
        assert_eq!(via_link, socket_path(Some(&tmp.path().join(suffix)), 501));
        assert!(via_link.as_os_str().len() <= 103);
    }
    #[test]
    fn racing_starters_recover_stale_socket_without_unlinking_live_primary() {
        let dir = tempfile::tempdir().unwrap();
        let endpoints = fixture_endpoints(dir.path());
        let path = &endpoints.socket;
        drop(UnixListener::bind(path).unwrap());
        wait_until_stale(path);
        let barrier = std::sync::Barrier::new(2);
        let (first, second) = std::thread::scope(|scope| {
            let start = || {
                let request = Request::new(&Options::default(), None).unwrap();
                barrier.wait();
                Instance::start_fixture(dir.path(), &request).unwrap()
            };
            let one = scope.spawn(start);
            let two = scope.spawn(start);
            (one.join().unwrap(), two.join().unwrap())
        });
        let (Start::Primary(primary), Start::Forwarded) = (if matches!(first, Start::Primary(_)) {
            (first, second)
        } else {
            (second, first)
        }) else {
            panic!("one primary and one forwarded")
        };
        assert_eq!(primary.drain().len(), 1);
        assert!(Claim::try_lock(&endpoints).unwrap().is_none());
        assert_eq!(path.metadata().unwrap().mode() & 0o777, 0o600);
    }
    #[test]
    fn legacy_socket_hash_matches_5497418_on_this_toolchain() {
        // Captured from the unmodified PathBuf/DefaultHasher implementation.
        let tmp = Path::new("/tmp/fixture");
        for (path, hash) in [
            (None, "bd60acb658c79e45"),
            (
                Some(Path::new("/private/tmp/velocity-legacy/profile")),
                "4d29c3c4468f0a51",
            ),
            (
                Some(Path::new("/tmp/../tmp/velocity-legacy/profile")),
                "64d5174ff0db2871",
            ),
        ] {
            assert_eq!(
                legacy_path(path, 501, tmp),
                tmp.join(format!("velocity-501-{hash}.sock"))
            );
        }
    }

    #[test]
    fn untrusted_socket_parent_uses_private_fallback() {
        let dir = tempfile::tempdir().unwrap();
        let blocked = dir.path().join("blocked");
        fs::DirBuilder::new().mode(0o755).create(&blocked).unwrap();
        let fallback = dir.path().join("fallback");
        fs::create_dir(&fallback).unwrap();
        // SAFETY: geteuid is a side-effect-free identity query.
        let uid = unsafe { libc::geteuid() };
        let actual =
            choose_endpoint(blocked.join("test.sock"), uid, || Ok(fallback.clone())).unwrap();
        assert_eq!(actual, fallback.join(format!("velocity-{uid}/test.sock")));
        let instance = UnixListener::bind(&actual).unwrap();
        assert!(UnixStream::connect(&actual).is_ok());
        drop(instance);
        assert!(!blocked.join("test.sock").exists());
    }

    #[test]
    fn deleted_socket_rebinds_and_forwards_to_the_same_primary() {
        let dir = tempfile::tempdir().unwrap();
        let endpoints = fixture_endpoints(dir.path());
        let request = Request::new(&Options::default(), None).unwrap();
        let Entry::Claimed(claim) = Instance::enter_at(&endpoints, &request).unwrap() else {
            panic!("primary")
        };
        let primary = claim.listen().unwrap();
        fs::remove_file(&endpoints.socket).unwrap();
        assert!(matches!(
            Instance::forward(&endpoints.socket, &request).unwrap(),
            Forward::NotRunning
        ));
        assert!(
            !endpoints
                .lock
                .starts_with(endpoints.socket.parent().unwrap())
        );
        assert!(Claim::try_lock(&endpoints).unwrap().is_none());
        assert!(matches!(
            Instance::enter_at(&endpoints, &request).unwrap(),
            Entry::Forwarded
        ));
        assert_eq!(primary.drain().len(), 1);
    }

    #[test]
    fn unreachable_locked_profile_reports_running_editor() {
        let dir = tempfile::tempdir().unwrap();
        let endpoints = fixture_endpoints(dir.path());
        let _lock = Claim::try_lock(&endpoints).unwrap().unwrap();
        let request = Request::new(&Options::default(), None).unwrap();
        let timeout = Duration::from_millis(50);
        // A crash marker is not evidence of an ongoing startup.
        let stale = File::create(Starting::path(&endpoints.socket)).unwrap();
        stale.set_modified(SystemTime::UNIX_EPOCH).unwrap();
        let started = Instant::now();
        let result = Instance::enter_until(&endpoints, &request, timeout, TIMEOUT);
        assert!(started.elapsed() >= timeout);
        assert_eq!(
            result.err().as_deref(),
            Some("the running editor is not responding")
        );
    }

    #[test]
    fn a_slow_starter_gets_a_longer_wait_and_then_receives_the_request() {
        let dir = tempfile::tempdir().unwrap();
        let endpoints = fixture_endpoints(dir.path());
        let request = Request::new(&Options::default(), None).unwrap();
        let Entry::Claimed(claim) = Instance::enter_at(&endpoints, &request).unwrap() else {
            panic!("expected launcher to hold the profile");
        };
        let marker = Starting::path(&endpoints.socket);
        assert!(marker.is_file());
        let ready = std::sync::Barrier::new(2);
        std::thread::scope(|scope| {
            let follower = scope.spawn(|| {
                ready.wait();
                Instance::enter_until(&endpoints, &request, Duration::from_millis(30), TIMEOUT)
            });
            ready.wait();
            // Longer than the regular locked-profile wait, shorter than the
            // cold-start grace period. No app or session is started by this test.
            std::thread::sleep(Duration::from_millis(150));
            let primary = claim.listen().unwrap();
            assert!(matches!(
                follower.join().unwrap().unwrap(),
                Entry::Forwarded
            ));
            assert!(!marker.exists());
            assert_eq!(primary.drain().len(), 1);
        });
        wait_until(|| Claim::try_lock(&endpoints).unwrap().is_some());
        let Entry::Claimed(launcher) = Instance::enter_at(&endpoints, &request).unwrap() else {
            panic!("expected another launcher");
        };
        assert!(marker.exists());
        drop(launcher);
        assert!(!marker.exists());
    }

    #[test]
    fn stale_legacy_socket_allows_a_new_primary() {
        let dir = tempfile::tempdir().unwrap();
        let mut endpoints = fixture_endpoints(dir.path());
        let stale = dir.path().join("legacy.sock");
        drop(UnixListener::bind(&stale).unwrap());
        wait_until_stale(&stale);
        endpoints.legacy.push(stale.clone());
        let marker = dir.path().join("foreign-marker");
        fs::write(&marker, "not a socket").unwrap();
        endpoints.legacy.push(marker.clone());
        // An option that the old peer would reject must still work after a crash.
        let options = Options::parse(["--command".to_owned(), "revert".to_owned()])
            .unwrap()
            .unwrap();
        let request = Request::new(&options, None).unwrap();
        let Start::Primary(primary) =
            Instance::start_entry(Instance::enter_at(&endpoints, &request).unwrap()).unwrap()
        else {
            panic!("stale endpoint prevented the primary from starting");
        };
        assert!(!stale.exists());
        assert_eq!(fs::read_to_string(marker).unwrap(), "not a socket");
        assert!(matches!(
            Instance::enter_at(&endpoints, &request).unwrap(),
            Entry::Forwarded
        ));
        assert_eq!(primary.drain().len(), 1);
    }

    #[test]
    fn seventeenth_modal_request_retries_until_enqueued_exactly_once() {
        let dir = tempfile::tempdir().unwrap();
        let endpoints = fixture_endpoints(dir.path());
        let mut request = Request::new(&Options::default(), None).unwrap();
        let Start::Primary(primary) = Instance::start_fixture(dir.path(), &request).unwrap() else {
            panic!("primary")
        };
        for _ in 0..16 {
            assert!(matches!(
                Instance::forward(&endpoints.socket, &request).unwrap(),
                Forward::Submitted(Ok(()))
            ));
        }
        let Forward::Submitted(Err(message)) = Instance::forward_with_ack(
            &endpoints.socket,
            &request,
            Duration::from_millis(60),
            || {},
        )
        .unwrap() else {
            panic!("full queue must report busy")
        };
        assert!(message.contains("the editor is busy"));
        request.add = true;
        let (finished, result) = mpsc::channel();
        std::thread::scope(|scope| {
            scope.spawn(|| {
                finished
                    .send(Instance::forward(&endpoints.socket, &request))
                    .unwrap();
            });
            assert!(result.recv_timeout(Duration::from_millis(100)).is_err());
            let mut drained = primary.drain();
            assert!(matches!(
                result
                    .recv_timeout(crate::test_timing::HANG_GUARD)
                    .unwrap()
                    .unwrap(),
                Forward::Submitted(Ok(()))
            ));
            drained.extend(primary.drain());
            assert_eq!(drained.len(), 17);
            assert_eq!(drained.iter().filter(|item| item.request.add).count(), 1);
        });
    }

    #[test]
    #[ignore = "child process driven by multiprocess_starters_share_one_profile"]
    fn child_entry_fixture() {
        let Some(root) = std::env::var_os("VELOCITY_RACE_FIXTURE") else {
            return;
        };
        let root = PathBuf::from(root);
        let id = std::env::var("VELOCITY_RACE_ID").unwrap();
        fs::write(root.join(format!("ready-{id}")), "").unwrap();
        wait_until(|| root.join("go").exists());
        let request = Request::new(&Options::default(), None).unwrap();
        match Instance::start(Some(&root.join("profile")), &request).unwrap() {
            Start::Primary(instance) => {
                let mut count = 0;
                wait_until(|| {
                    count += instance.drain().len();
                    count == 2
                });
                assert_eq!(count, 2);
                // Only now is it safe for the parent to stop the children: a
                // forwarder reports itself once its request is sent, not once
                // the primary has read it, so stopping earlier can cut the
                // second request off and make this child exit non-zero.
                fs::write(root.join(format!("primary-{id}")), "").unwrap();
                wait_until(|| root.join("stop").exists());
            }
            Start::Forwarded => fs::write(root.join(format!("forwarded-{id}")), "").unwrap(),
        }
    }

    /// A socket that this process bound and closed can stay connectable for a
    /// moment: a child forked by another test thread inherits the listening
    /// descriptor until it execs, and under load that window is long enough to
    /// be seen. Wait until the endpoint really refuses connections, which is
    /// what these tests mean by a stale socket.
    fn wait_until_stale(path: &Path) {
        let until = Instant::now() + crate::test_timing::HANG_GUARD;
        while UnixStream::connect(path).is_ok() {
            assert!(
                Instant::now() < until,
                "{} kept accepting connections",
                path.display()
            );
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    fn wait_until(mut ready: impl FnMut() -> bool) {
        let until = Instant::now() + crate::test_timing::HANG_GUARD;
        while !ready() {
            assert!(Instant::now() < until, "process fixture timed out");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn multiprocess_starters_share_one_profile() {
        for stale in (0..24).map(|i| i % 2 == 1) {
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path();
            let endpoints = fixture_endpoints(root);
            if stale {
                drop(UnixListener::bind(&endpoints.socket).unwrap());
            }
            let mut children: Vec<_> = (0..3)
                .map(|id| {
                    std::process::Command::new(std::env::current_exe().unwrap())
                        .args([
                            "--ignored",
                            "--exact",
                            "instance::tests::child_entry_fixture",
                        ])
                        .env("HOME", root)
                        .env("SHELL", "/bin/sh")
                        .env("TMPDIR", root)
                        .env("VELOCITY_INSTANCE_SOCKET_DIR", root.join("ipc"))
                        .env("VELOCITY_RACE_FIXTURE", root)
                        .env("VELOCITY_RACE_ID", id.to_string())
                        .stdout(std::process::Stdio::null())
                        .spawn()
                        .unwrap()
                })
                .collect();
            wait_until(|| (0..3).all(|id| root.join(format!("ready-{id}")).exists()));
            fs::write(root.join("go"), "").unwrap();
            wait_until(|| {
                (0..3).all(|id| {
                    ["primary", "forwarded"]
                        .iter()
                        .any(|role| root.join(format!("{role}-{id}")).exists())
                })
            });
            let primary_count = (0..3)
                .filter(|id| root.join(format!("primary-{id}")).exists())
                .count();
            fs::write(root.join("stop"), "").unwrap();
            for child in &mut children {
                assert!(child.wait().unwrap().success());
            }
            assert_eq!(primary_count, 1);
        }
    }
    #[test]
    fn older_cli_requests_omit_native_marker_and_native_requests_roundtrip() {
        let mut value = serde_json::json!({"files":[],"project":null,"new_window":false,"add":false,"wait":false,"stdin":null});
        let legacy: super::Request = serde_json::from_value(value.clone()).unwrap();
        assert!(!legacy.native_open);
        assert!(
            !legacy.background
                && !legacy.stay
                && legacy.command.is_none()
                && legacy.return_pid.is_none()
        );
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
        let options = Options {
            wait: true,
            ..Options::default()
        };
        let request = Request::new(&options, None).unwrap();
        let Start::Primary(instance) = Instance::start_fixture(dir.path(), &request).unwrap()
        else {
            panic!("expected primary")
        };
        // Binding is synchronous in start(). Prepare the expensive workspace before
        // starting the client's acknowledgement timeout. Other workspace tests may
        // concurrently compile the full syntax corpus.
        let mut workspace = Workspace::new(vel_text::Text::new(), None, None, false).unwrap();
        let target = fixture_endpoints(dir.path()).socket;
        let (accepted_tx, accepted_rx) = mpsc::channel();
        let (closed_tx, closed_rx) = mpsc::channel();
        let client = std::thread::spawn(move || {
            let result = Instance::forward_with_ack(&target, &request, TIMEOUT, || {
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
        assert!(matches!(
            closed_rx
                .recv_timeout(watchdog)
                .expect("close acknowledgement")
                .unwrap(),
            Forward::Submitted(Ok(()))
        ));
        client.join().unwrap();
    }
}

#[cfg(all(test, unix))]
mod round15_tests {
    use super::*;
    use vel_text::Text;
    #[test]
    fn confirmed_request_expires_without_a_consumer_and_frees_ingress() {
        let queue = Ingress::start(1).unwrap();
        let request = Request::new(&Options::default(), None).unwrap();
        let (server, mut client) = UnixStream::pair().unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        assert_eq!(
            queue.0.push(Incoming::local(request.clone()), MAX_REQUEST),
            Admission::Accepted
        );
        assert_eq!(
            queue.0.push(Incoming::local(request.clone()), 1),
            Admission::Full
        );
        assert!(queue.next().is_some());
        assert_eq!(
            queue.0.push(
                Incoming::confirmed_fixture(request.clone(), server, Duration::from_millis(40)),
                MAX_REQUEST,
            ),
            Admission::Accepted
        );
        let mut response = String::new();
        client.read_to_string(&mut response).unwrap();
        assert_eq!(response, format!("\x03{DELIVERY_EXPIRED}"));
        assert!(queue.next().is_none());
        assert_eq!(
            queue.0.push(Incoming::local(request), MAX_REQUEST),
            Admission::Accepted
        );
        assert!(queue.next().is_some());
    }

    #[test]
    fn explicit_busy_retries_but_delivery_failure_does_not() {
        let request = Request::new(&Options::default(), None).unwrap();
        let (first, mut busy) = UnixStream::pair().unwrap();
        let (second, mut reject) = UnixStream::pair().unwrap();
        let thread = std::thread::spawn(move || {
            let read = |stream: &mut UnixStream| {
                let mut len = [0; 8];
                stream.read_exact(&mut len).unwrap();
                let mut bytes = vec![0; u64::from_le_bytes(len) as usize];
                stream.read_exact(&mut bytes).unwrap();
                bytes
            };
            let body = read(&mut busy);
            busy.write_all(&[BUSY]).unwrap();
            assert_eq!(read(&mut reject), body);
            reject.write_all(b"\x03destination closed").unwrap();
        });
        let second = std::cell::RefCell::new(Some(second));
        let attempts = std::cell::Cell::new(0);
        let error = submit(
            first,
            &request,
            Duration::from_secs(5),
            || {
                let attempt = attempts.replace(attempts.get() + 1);
                (attempt > 0).then(|| second.borrow_mut().take().expect("one replay after BUSY"))
            },
            || panic!("must not acknowledge a rejected request"),
            None,
        )
        .unwrap_err();
        assert!(error.contains("destination closed"));
        assert_eq!(attempts.get(), 2);
        thread.join().unwrap();
    }
    #[test]
    fn unresponsive_primary_owns_submitted_request_even_without_an_ack() {
        let dir = tempfile::tempdir().unwrap();
        let path = fixture_endpoints(dir.path()).socket;
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
            resume.recv_timeout(crate::test_timing::HANG_GUARD).unwrap();
            // The primary may still apply this queued request after the client returns.
        });
        let mut request = Request::new(&Options::default(), None).unwrap();
        request.native_open = true;
        assert!(matches!(
            Instance::forward_with_ack(&path, &request, Duration::from_millis(100), || {}).unwrap(),
            Forward::Submitted(Err(_))
        ));
        assert!(
            received
                .recv_timeout(crate::test_timing::HANG_GUARD)
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
        client
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut w = Workspace::new(Text::new(), None, None, false).unwrap();
        w.editor_mut()
            .run("insert", &serde_json::json!({"characters":"original"}))
            .unwrap();
        let sheet = w.tabs[0].sheet;
        w.stdout_sheets.insert(sheet);
        let mut waiter =
            Waiter::acknowledge(Incoming::fixture(request, stream), &mut w, [sheet].into())
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
        assert_eq!(client.read(&mut ack).unwrap(), 0);
    }
    #[test]
    fn window_close_returns_latest_stdin_and_native_ack_does_not_wait_for_ui() {
        let dir = tempfile::tempdir().unwrap();
        let request = Request::new(&Options::default(), None).unwrap();
        let Start::Primary(instance) = Instance::start_fixture(dir.path(), &request).unwrap()
        else {
            panic!("primary")
        };
        let mut native = request;
        native.native_open = true;
        let path = fixture_endpoints(dir.path()).socket;
        let client = std::thread::spawn(move || Instance::forward(&path, &native));
        // No App/GUI has consumed a request, yet the listener already accepted it.
        assert!(matches!(
            client.join().unwrap().unwrap(),
            Forward::Submitted(Ok(()))
        ));
        let incoming = instance
            .rx
            .recv_timeout(crate::test_timing::HANG_GUARD)
            .unwrap();
        assert!(incoming.request.native_open);
        let mut w = Workspace::new(Text::new(), None, None, false).unwrap();
        assert!(
            Waiter::acknowledge(incoming, &mut w, Default::default())
                .unwrap()
                .is_none()
        );
        let (stream, mut client) = UnixStream::pair().unwrap();
        let request = Request::new(&Options::default(), Some("".into())).unwrap();
        let sheet = w.tabs[0].sheet;
        w.stdout_sheets.insert(sheet);
        let mut waiter =
            Waiter::acknowledge(Incoming::fixture(request, stream), &mut w, [sheet].into())
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
