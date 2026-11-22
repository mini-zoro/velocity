//! Windows primary listener and instance election.
use super::{BUSY, ENQUEUED, Entry, Forward, Incoming, MAX_REQUEST, Request, TIMEOUT};
use crate::instance_pipe::{Endpoint, Pipe, code};
use std::{
    io::{self, Read, Write},
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};
use windows_sys::Win32::Foundation::{ERROR_ACCESS_DENIED, ERROR_PIPE_BUSY};

#[derive(Debug)]
pub struct Claim {
    endpoint: Endpoint,
    first: Pipe,
}
impl Claim {
    pub fn listen(self) -> io::Result<Instance> {
        let (tx, rx) = mpsc::sync_channel(32);
        let stop = Arc::new(AtomicBool::new(false));
        let worker = stop.clone();
        std::thread::Builder::new()
            .name("velocity-instance".into())
            .spawn(move || serve(self, tx, &worker))?;
        Ok(Instance { rx, stop })
    }
}

fn serve(
    Claim {
        endpoint,
        mut first,
    }: Claim,
    tx: mpsc::SyncSender<Incoming>,
    stop: &AtomicBool,
) {
    while !stop.load(Ordering::Acquire) {
        let connected = first.accept(stop);
        if stop.load(Ordering::Acquire) {
            break;
        }
        // Reserve the next listener before releasing this pipe. There must always
        // be a named instance while the primary owns this profile.
        let next = loop {
            if stop.load(Ordering::Acquire) {
                return;
            }
            match endpoint.listen(false) {
                Ok(pipe) => break pipe,
                Err(_) => std::thread::sleep(Duration::from_millis(50)),
            }
        };
        let mut stream = std::mem::replace(&mut first, next);
        if connected.is_err() {
            continue;
        }
        let until = Instant::now() + Duration::from_secs(5);
        let _ = stream.set_write_timeout(Some(Duration::from_millis(100)));
        let read = |stream: &mut Pipe, mut bytes: &mut [u8]| {
            while !bytes.is_empty() {
                let left = until.saturating_duration_since(Instant::now());
                if stop.load(Ordering::Acquire) || left.is_zero() {
                    return Err(io::ErrorKind::TimedOut.into());
                }
                stream.set_read_timeout(Some(left.min(Duration::from_millis(100))))?;
                match stream.read(bytes) {
                    Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
                    Ok(n) => bytes = &mut bytes[n..],
                    Err(e) if e.kind() == io::ErrorKind::TimedOut => {}
                    Err(e) => return Err(e),
                }
            }
            Ok(())
        };
        let mut len = [0; 8];
        if read(&mut stream, &mut len).is_err() {
            continue;
        }
        let len = u64::from_le_bytes(len);
        if len > MAX_REQUEST {
            continue;
        }
        let mut bytes = vec![0; len as usize];
        if read(&mut stream, &mut bytes).is_err() {
            continue;
        }
        let Ok(request) = serde_json::from_slice::<Request>(&bytes) else {
            continue;
        };
        let (reply, ready) = mpsc::sync_channel(1);
        let wait = request.wait;
        match tx.try_send(Incoming {
            request,
            reply: wait.then_some(ready),
        }) {
            Ok(()) => {
                if stream.write_all(&[ENQUEUED]).is_ok() && wait {
                    let _ = reply.send(stream);
                }
            }
            Err(mpsc::TrySendError::Full(_)) => {
                let _ = stream.write_all(&[BUSY]);
            }
            Err(mpsc::TrySendError::Disconnected(_)) => break,
        }
    }
}

#[derive(Debug)]
pub struct Instance {
    rx: mpsc::Receiver<Incoming>,
    stop: Arc<AtomicBool>,
}
pub enum Start {
    Forwarded,
    Primary(Instance),
}
impl Instance {
    pub fn enter(data: Option<&Path>, request: &Request) -> Result<Entry, String> {
        let endpoint = Endpoint::for_profile(data).map_err(|e| e.to_string())?;
        let deadline = Instant::now() + TIMEOUT;
        loop {
            match endpoint.forward(request)? {
                Forward::Submitted(result) => return result.map(|()| Entry::Forwarded),
                Forward::NotRunning => match endpoint.listen(true) {
                    Ok(first) => return Ok(Entry::Claimed(Claim { endpoint, first })),
                    Err(e) if matches!(code(&e), Some(ERROR_ACCESS_DENIED | ERROR_PIPE_BUSY)) => {}
                    Err(e) => return Err(format!("cannot claim instance pipe: {e}")),
                },
            }
            if Instant::now() >= deadline {
                return Err(
                    "the editor's local pipe is unavailable; quit the running editor first".into(),
                );
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    /// The console client will send the request once this server is listening.
    pub(crate) fn claim_startup(data: Option<&Path>) -> Result<Option<Claim>, String> {
        let endpoint = Endpoint::for_profile(data).map_err(|e| e.to_string())?;
        match endpoint.listen(true) {
            Ok(first) => Ok(Some(Claim { endpoint, first })),
            Err(e) if matches!(code(&e), Some(ERROR_ACCESS_DENIED | ERROR_PIPE_BUSY)) => Ok(None),
            Err(e) => Err(format!("cannot claim instance pipe: {e}")),
        }
    }
    pub fn start(data: Option<&Path>, request: &Request) -> Result<Start, String> {
        match Self::enter(data, request)? {
            Entry::Forwarded => Ok(Start::Forwarded),
            Entry::Claimed(claim) => claim
                .listen()
                .map(Start::Primary)
                .map_err(|e| e.to_string()),
        }
    }
    pub fn drain(&self) -> Vec<Incoming> {
        self.rx.try_iter().collect()
    }
}
impl Drop for Instance {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::{FileLocation, Options};
    use std::path::PathBuf;

    #[test]
    fn one_primary_receives_files_with_native_paths_per_profile() {
        let sandbox = tempfile::tempdir().unwrap();
        let options = Options {
            files: vec![
                FileLocation::parse(r"C:\work\new.rs:20:6"),
                FileLocation::path_only(PathBuf::from(r"\\server\share\notes.txt")),
            ],
            new_window: true,
            ..Options::default()
        };
        let request = Request::new(&options, None).unwrap();
        let profile = sandbox.path().join("test Profile");
        let alias = sandbox.path().join("TEST PROFILE");
        assert_eq!(
            Endpoint::for_profile(Some(&profile)).unwrap().name,
            Endpoint::for_profile(Some(&alias)).unwrap().name
        );
        let Start::Primary(primary) = Instance::start(Some(&profile), &request).unwrap() else {
            panic!("profile already running");
        };
        assert!(matches!(
            Instance::start(Some(&alias), &request).unwrap(),
            Start::Forwarded
        ));
        let incoming = primary.drain().pop().unwrap();
        assert_eq!(incoming.request.files[0].path, Path::new(r"C:\work\new.rs"));
        assert_eq!(
            incoming.request.files[1].path,
            Path::new(r"\\server\share\notes.txt")
        );
        assert_eq!(incoming.request.files[0].line, 20);
        assert_eq!(incoming.request.files[0].column, 6);
        assert!(incoming.request.new_window);
        let another = sandbox.path().join("Another Profile");
        assert!(matches!(
            Instance::start(Some(&another), &request).unwrap(),
            Start::Primary(_)
        ));
    }

    #[test]
    fn wait_stream_has_distinct_enqueue_and_close_replies() {
        let sandbox = tempfile::tempdir().unwrap();
        let path = Some(sandbox.path());
        let request = Request::new(
            &Options {
                wait: true,
                ..Options::default()
            },
            None,
        )
        .unwrap();
        let Start::Primary(primary) = Instance::start(path, &request).unwrap() else {
            panic!("primary");
        };
        let mut stream = Endpoint::for_profile(path).unwrap().connect().unwrap();
        stream.set_read_timeout(Some(TIMEOUT)).unwrap();
        stream.set_write_timeout(Some(TIMEOUT)).unwrap();
        let bytes = serde_json::to_vec(&request).unwrap();
        stream
            .write_all(&(bytes.len() as u64).to_le_bytes())
            .unwrap();
        stream.write_all(&bytes).unwrap();
        let mut ack = [0];
        stream.read_exact(&mut ack).unwrap();
        assert_eq!(ack, [ENQUEUED]);
        let mut incoming = primary.drain().pop().unwrap();
        let mut reply = incoming
            .reply
            .take()
            .unwrap()
            .recv_timeout(TIMEOUT)
            .unwrap();
        reply.set_write_timeout(Some(TIMEOUT)).unwrap();
        reply.write_all(&[2]).unwrap();
        drop(reply);
        stream.read_exact(&mut ack).unwrap();
        assert_eq!(ack, [2]);
        assert_eq!(stream.read(&mut ack).unwrap(), 0);
    }
}
