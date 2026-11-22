//! Windows local named pipe, shared by the GUI host and console client.
use crate::instance_protocol::{Forward, Request, TIMEOUT, submit};
use std::{
    cell::Cell,
    io::{self, Read, Write},
    os::windows::{
        ffi::OsStrExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    },
    path::{Path, PathBuf},
    ptr::{null, null_mut},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::{
        ERROR_BROKEN_PIPE, ERROR_FILE_NOT_FOUND, ERROR_IO_PENDING, ERROR_NO_DATA, ERROR_PIPE_BUSY,
        ERROR_PIPE_CONNECTED, ERROR_PIPE_NOT_CONNECTED, ERROR_SUCCESS, HANDLE,
        INVALID_HANDLE_VALUE, LocalFree, WAIT_OBJECT_0, WAIT_TIMEOUT,
    },
    Security::{
        Authorization::{
            ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
            GetSecurityInfo, SE_KERNEL_OBJECT,
        },
        GetTokenInformation, OWNER_SECURITY_INFORMATION, PSID, SECURITY_ATTRIBUTES, TOKEN_QUERY,
        TOKEN_USER, TokenUser,
    },
    Storage::FileSystem::{
        CreateFileW, FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED, FILE_READ_DATA,
        FILE_WRITE_DATA, OPEN_EXISTING, PIPE_ACCESS_DUPLEX, READ_CONTROL, ReadFile,
        SECURITY_IDENTIFICATION, SECURITY_SQOS_PRESENT, SYNCHRONIZE, WriteFile,
    },
    System::{
        IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED},
        Pipes::{
            ConnectNamedPipe, CreateNamedPipeW, PIPE_REJECT_REMOTE_CLIENTS,
            PIPE_UNLIMITED_INSTANCES,
        },
        Threading::{
            CreateEventW, GetCurrentProcess, INFINITE, OpenProcessToken, WaitForSingleObject,
        },
    },
};

pub(crate) fn code(error: &io::Error) -> Option<u32> {
    error.raw_os_error().map(|value| value as u32)
}
fn wide(path: &Path) -> Vec<u16> {
    path.as_os_str().encode_wide().chain([0]).collect()
}

struct Local(*mut std::ffi::c_void);
impl Drop for Local {
    fn drop(&mut self) {
        // SAFETY: this pointer is null or was allocated by a Win32 LocalAlloc API.
        unsafe { LocalFree(self.0) };
    }
}

fn sid_string(sid: PSID) -> io::Result<String> {
    let mut string = null_mut();
    // SAFETY: callers pass a valid SID; Win32 provides a NUL-terminated allocated string.
    if unsafe { ConvertSidToStringSidW(sid, &mut string) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let _allocation = Local(string.cast());
    // SAFETY: ConvertSidToStringSidW returned a NUL-terminated UTF-16 string.
    let units = unsafe {
        let len = (0..).take_while(|&i| *string.add(i) != 0).count();
        std::slice::from_raw_parts(string, len)
    };
    Ok(String::from_utf16_lossy(units))
}

fn current_sid() -> io::Result<String> {
    let mut token = null_mut();
    // SAFETY: current process is a valid pseudo-handle; token is a writable output.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: OpenProcessToken returned a fresh owned handle.
    let token = unsafe { OwnedHandle::from_raw_handle(token) };
    let mut needed = 0;
    // SAFETY: the null buffer and zero length query the required capacity.
    unsafe { GetTokenInformation(token.as_raw_handle(), TokenUser, null_mut(), 0, &mut needed) };
    if needed == 0 {
        return Err(io::Error::last_os_error());
    }
    let mut storage = vec![0usize; (needed as usize).div_ceil(size_of::<usize>())];
    // SAFETY: storage is word-aligned and has capacity for needed bytes.
    if unsafe {
        GetTokenInformation(
            token.as_raw_handle(),
            TokenUser,
            storage.as_mut_ptr().cast(),
            needed,
            &mut needed,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: a successful TokenUser query starts with TOKEN_USER and stores its SID in storage.
    sid_string(unsafe { (*storage.as_ptr().cast::<TOKEN_USER>()).User.Sid })
}

#[derive(Debug)]
pub(crate) struct Endpoint {
    pub(crate) name: PathBuf,
    sid: String,
}
impl Endpoint {
    pub(crate) fn for_profile(data: Option<&Path>) -> io::Result<Self> {
        let sid = current_sid()?;
        let mut hash = 0xcbf29ce484222325_u64;
        if let Some(path) = data {
            let canonical = crate::instance_path::canonical_profile(&std::path::absolute(path)?);
            let spelling = vel_config::paths::display_path(&canonical);
            for unit in spelling
                .as_os_str()
                .to_string_lossy()
                .replace('/', "\\")
                .to_uppercase()
                .encode_utf16()
            {
                hash = (hash ^ u64::from(unit)).wrapping_mul(0x100000001b3);
            }
        }
        Ok(Self {
            name: PathBuf::from(format!(r"\\.\pipe\Velocity-{sid}-{hash:016x}")),
            sid,
        })
    }

    pub(crate) fn listen(&self, first: bool) -> io::Result<Pipe> {
        let name = wide(&self.name);
        let sddl: Vec<u16> = format!("O:{0}D:P(A;;GA;;;{0})", self.sid)
            .encode_utf16()
            .chain([0])
            .collect();
        let mut descriptor = Local(null_mut());
        // SAFETY: sddl is terminated and the output is released with LocalFree.
        if unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                1,
                &mut descriptor.0,
                null_mut(),
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        let attributes = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor.0,
            bInheritHandle: 0,
        };
        let flags = PIPE_ACCESS_DUPLEX
            | FILE_FLAG_OVERLAPPED
            | if first {
                FILE_FLAG_FIRST_PIPE_INSTANCE
            } else {
                0
            };
        // SAFETY: name and security descriptor are valid throughout creation; no pointer is retained.
        let pipe = unsafe {
            CreateNamedPipeW(
                name.as_ptr(),
                flags,
                PIPE_REJECT_REMOTE_CLIENTS,
                PIPE_UNLIMITED_INSTANCES,
                65536,
                65536,
                0,
                &attributes,
            )
        };
        Pipe::new(pipe)
    }

    pub(crate) fn connect(&self) -> io::Result<Pipe> {
        let name = wide(&self.name);
        // SAFETY: this opens an existing pipe. SQOS prevents a server from
        // impersonating a connecting process, even when it has guessed the name.
        let handle = unsafe {
            CreateFileW(
                name.as_ptr(),
                FILE_READ_DATA | FILE_WRITE_DATA | READ_CONTROL | SYNCHRONIZE,
                0,
                null(),
                OPEN_EXISTING,
                FILE_FLAG_OVERLAPPED | SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION,
                null_mut(),
            )
        };
        let pipe = Pipe::new(handle)?;
        let mut owner = null_mut();
        let mut descriptor = Local(null_mut());
        // SAFETY: owner points into the returned descriptor, which remains alive through the check.
        let result = unsafe {
            GetSecurityInfo(
                pipe.handle.as_raw_handle(),
                SE_KERNEL_OBJECT,
                OWNER_SECURITY_INFORMATION,
                &mut owner,
                null_mut(),
                null_mut(),
                null_mut(),
                &mut descriptor.0,
            )
        };
        if result != ERROR_SUCCESS {
            return Err(io::Error::from_raw_os_error(result as i32));
        }
        if owner.is_null() || sid_string(owner)? != self.sid {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "instance pipe belongs to another user",
            ));
        }
        Ok(pipe)
    }
}

#[derive(Debug)]
pub(crate) struct Pipe {
    handle: OwnedHandle,
    event: OwnedHandle,
    read_timeout: Cell<Option<Duration>>,
    write_timeout: Cell<Option<Duration>>,
}
impl Pipe {
    fn new(handle: HANDLE) -> io::Result<Self> {
        if handle == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: the pipe create/open call transferred ownership on success.
        let handle = unsafe { OwnedHandle::from_raw_handle(handle) };
        // SAFETY: null attributes and name create a private manual-reset event.
        let event = unsafe { CreateEventW(null(), 1, 0, null()) };
        if event.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: CreateEventW transferred ownership on success.
        let event = unsafe { OwnedHandle::from_raw_handle(event) };
        Ok(Self {
            handle,
            event,
            read_timeout: Cell::new(None),
            write_timeout: Cell::new(None),
        })
    }

    fn set_timeout(cell: &Cell<Option<Duration>>, value: Option<Duration>) -> io::Result<()> {
        if value.is_some_and(|duration| duration.is_zero()) {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        cell.set(value);
        Ok(())
    }
    pub(crate) fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        Self::set_timeout(&self.read_timeout, timeout)
    }
    pub(crate) fn set_write_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        Self::set_timeout(&self.write_timeout, timeout)
    }

    fn perform(
        &mut self,
        timeout: Option<Duration>,
        stop: Option<&AtomicBool>,
        start: impl FnOnce(HANDLE, &mut OVERLAPPED) -> i32,
    ) -> io::Result<usize> {
        let handle = self.handle.as_raw_handle();
        let mut request = OVERLAPPED {
            hEvent: self.event.as_raw_handle(),
            ..Default::default()
        };
        let mut count = 0;
        if start(handle, &mut request) == 0 {
            let error = io::Error::last_os_error();
            match code(&error) {
                Some(ERROR_PIPE_CONNECTED) => return Ok(0),
                Some(ERROR_IO_PENDING) => {}
                _ => return Err(error),
            }
            let deadline = timeout.map(|duration| Instant::now() + duration);
            loop {
                let left = deadline.map(|at| at.saturating_duration_since(Instant::now()));
                let cancelled = stop.is_some_and(|stop| stop.load(Ordering::Acquire));
                let millis = left.map_or(INFINITE, |d| {
                    d.as_millis().clamp(1, u128::from(INFINITE - 1)) as u32
                });
                let millis = if stop.is_some() {
                    millis.min(100)
                } else {
                    millis
                };
                let state = if cancelled {
                    WAIT_TIMEOUT
                } else {
                    // SAFETY: event is live and belongs exclusively to this overlapped operation.
                    unsafe { WaitForSingleObject(request.hEvent, millis) }
                };
                if state == WAIT_OBJECT_0 {
                    break;
                }
                if state == WAIT_TIMEOUT
                    && !cancelled
                    && deadline.is_none_or(|at| Instant::now() < at)
                {
                    continue;
                }
                let error = if state == WAIT_TIMEOUT {
                    io::ErrorKind::TimedOut.into()
                } else {
                    io::Error::last_os_error()
                };
                // SAFETY: cancellation alone does not retire an overlapped buffer.
                // Wait for completion before reclaiming either it or the caller's data.
                let completed = unsafe {
                    CancelIoEx(handle, &request);
                    GetOverlappedResult(handle, &request, &mut count, 1)
                };
                // Completion can win the timeout race; retain bytes already transferred.
                if completed != 0 && !cancelled {
                    return Ok(count as usize);
                }
                return Err(error);
            }
        }
        // SAFETY: request completed or signaled its event and count is writable.
        if unsafe { GetOverlappedResult(handle, &request, &mut count, 0) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(count as usize)
    }

    pub(crate) fn accept(&mut self, stop: &AtomicBool) -> io::Result<()> {
        self.perform(None, Some(stop), |pipe, request| {
            // SAFETY: request is retained by perform until the connection operation retires.
            unsafe { ConnectNamedPipe(pipe, request) }
        })
        .map(|_| ())
    }
}

impl Read for Pipe {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        let len = buffer.len().min(65536) as u32;
        let result = self.perform(self.read_timeout.get(), None, |pipe, request| {
            // SAFETY: the sized buffer and overlapped request live until the I/O retires.
            unsafe { ReadFile(pipe, buffer.as_mut_ptr(), len, null_mut(), request) }
        });
        match result {
            Err(error)
                if matches!(
                    code(&error),
                    Some(ERROR_BROKEN_PIPE | ERROR_NO_DATA | ERROR_PIPE_NOT_CONNECTED)
                ) =>
            {
                Ok(0)
            }
            result => result,
        }
    }
}
impl Write for Pipe {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        let len = buffer.len().min(65536) as u32;
        self.perform(self.write_timeout.get(), None, |pipe, request| {
            // SAFETY: the sized buffer and overlapped request live until the I/O retires.
            unsafe { WriteFile(pipe, buffer.as_ptr(), len, null_mut(), request) }
        })
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    } // Writes are not buffered in this wrapper.
}

impl Endpoint {
    pub(crate) fn forward(&self, request: &Request) -> Result<Forward, String> {
        let stream = match self.connect() {
            Ok(stream) => stream,
            Err(e)
                if matches!(
                    code(&e),
                    Some(ERROR_FILE_NOT_FOUND | ERROR_PIPE_BUSY | ERROR_NO_DATA)
                ) =>
            {
                return Ok(Forward::NotRunning);
            }
            Err(e) => return Err(format!("cannot connect to the editor: {e}")),
        };
        Ok(Forward::Submitted(submit(
            stream,
            request,
            TIMEOUT,
            || self.connect().ok(),
            || {},
            None,
        )))
    }
}
