//! Reads a decoder child's reply with a strict byte limit and deadline.
use std::{
    io::Read,
    sync::atomic::{AtomicBool, Ordering},
};

pub(crate) fn bounded_reply(
    command: &mut std::process::Command,
    cancelled: &AtomicBool,
    budget: std::time::Duration,
    max_output: usize,
) -> Option<Vec<u8>> {
    use std::{io::ErrorKind, os::fd::AsRawFd, process::Stdio, time::Instant};
    let deadline = Instant::now() + budget;
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut pipe = child.stdout.take().expect("decoder stdout");
    let fd = pipe.as_raw_fd();
    let mut read = || {
        // SAFETY: pipe owns fd; only its status flags are read and changed.
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        // SAFETY: fd is still owned by the same pipe, which is only used here.
        if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
            return None;
        }
        let mut data = Vec::new();
        let mut chunk = [0u8; 16 * 1024];
        let mut eof = false;
        while !cancelled.load(Ordering::Acquire) && Instant::now() < deadline {
            if eof {
                if let Some(status) = child.try_wait().ok()? {
                    return status.success().then_some(data);
                }
            } else {
                match pipe.read(&mut chunk) {
                    Ok(0) => eof = true,
                    Ok(n) if data.len() + n <= max_output => {
                        data.extend_from_slice(&chunk[..n]);
                        continue;
                    }
                    Err(e)
                        if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::Interrupted) => {}
                    _ => return None,
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        None
    };
    let result = read();
    if result.is_none() {
        let _ = child.kill();
        let _ = child.wait();
    }
    result
}
