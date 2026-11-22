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
    use std::{io::ErrorKind, process::Stdio, time::Instant};
    let deadline = Instant::now() + budget;
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut pipe = child.stdout.take().expect("decoder stdout");
    let mut read = || {
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            let fd = pipe.as_raw_fd();
            // SAFETY: pipe owns fd; only its status flags are read and changed.
            let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
            // SAFETY: fd is still owned by the same pipe, which is only used here.
            if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0
            {
                return None;
            }
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
                #[cfg(unix)]
                let reply = pipe.read(&mut chunk);
                #[cfg(windows)]
                let reply = read_ready(&mut pipe, &mut chunk);
                match reply {
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

#[cfg(windows)]
fn read_ready(pipe: &mut std::process::ChildStdout, chunk: &mut [u8]) -> std::io::Result<usize> {
    use std::{
        io::{Error, ErrorKind},
        os::windows::io::AsRawHandle,
        ptr::null_mut,
    };
    use windows_sys::Win32::{Foundation::ERROR_BROKEN_PIPE, System::Pipes::PeekNamedPipe};
    let mut available = 0;
    // SAFETY: this is the only reader; the handle and output count are live.
    let ok = unsafe {
        PeekNamedPipe(
            pipe.as_raw_handle(),
            null_mut(),
            0,
            null_mut(),
            &mut available,
            null_mut(),
        )
    };
    if ok == 0 {
        let error = Error::last_os_error();
        if error.raw_os_error() == Some(ERROR_BROKEN_PIPE as i32) {
            return Ok(0);
        }
        return Err(error);
    }
    if available == 0 {
        return Err(ErrorKind::WouldBlock.into());
    }
    let len = chunk.len().min(available as usize);
    pipe.read(&mut chunk[..len])
}
