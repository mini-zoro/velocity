//! Start the sibling GUI without inheriting the console client's handles.
use std::{
    ffi::OsStr,
    io,
    mem::size_of,
    os::windows::{
        ffi::OsStrExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
        process::ExitStatusExt,
    },
    process::{Command, ExitStatus},
    ptr::null,
};
use windows_sys::Win32::{
    Foundation::{WAIT_OBJECT_0, WAIT_TIMEOUT},
    System::Threading::{
        CreateProcessW, DETACHED_PROCESS, GetExitCodeProcess, PROCESS_INFORMATION, STARTUPINFOW,
        WaitForSingleObject,
    },
};

fn argument(line: &mut Vec<u16>, arg: &OsStr) -> io::Result<()> {
    // Match the Windows CRT rules: double backslashes before a literal quote
    // or the closing delimiter. Keep the original UTF-16 code units.
    const QUOTE: u16 = b'"' as u16;
    const SLASH: u16 = b'\\' as u16;
    if !line.is_empty() {
        line.push(b' ' as u16);
    }
    line.push(QUOTE);
    let mut slashes = 0;
    for unit in arg.encode_wide() {
        if unit == 0 {
            return Err(io::Error::from(io::ErrorKind::InvalidInput));
        }
        if unit == SLASH {
            slashes += 1;
            continue;
        }
        line.extend(std::iter::repeat_n(
            SLASH,
            if unit == QUOTE {
                2 * slashes + 1
            } else {
                slashes
            },
        ));
        slashes = 0;
        line.push(unit);
    }
    line.extend(std::iter::repeat_n(SLASH, 2 * slashes));
    line.push(QUOTE);
    Ok(())
}

pub(crate) struct GuiProcess(OwnedHandle);

impl GuiProcess {
    pub(crate) fn try_wait(&self) -> io::Result<Option<ExitStatus>> {
        // SAFETY: The handle is owned and WaitForSingleObject does not consume it.
        match unsafe { WaitForSingleObject(self.0.as_raw_handle(), 0) } {
            WAIT_OBJECT_0 => {
                let mut code = 0;
                // SAFETY: The exited process remains referenced and code is writable.
                if unsafe { GetExitCodeProcess(self.0.as_raw_handle(), &mut code) } == 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(Some(ExitStatus::from_raw(code)))
            }
            WAIT_TIMEOUT => Ok(None),
            _ => Err(io::Error::last_os_error()),
        }
    }
}

/// Uses the program and arguments from the builder; inherits cwd and environment.
pub(crate) fn spawn(command: &Command) -> io::Result<GuiProcess> {
    let prog = command.get_program();
    debug_assert!(std::path::Path::new(prog).is_absolute());
    let mut line = Vec::new();
    argument(&mut line, prog)?;
    for arg in command.get_args() {
        argument(&mut line, arg)?;
    }
    line.push(0);
    let exe: Vec<u16> = prog.encode_wide().chain([0]).collect();

    // Stdio::null() replaces the three standard stream slots, but the stable
    // std Windows launcher can still inherit other inheritable handles, including
    // the old redirected output handles. Explicitly disable *all* inheritance.
    // The detached GUI has no console to fill the empty standard handle slots.
    let startup = STARTUPINFOW {
        cb: size_of::<STARTUPINFOW>() as u32,
        ..Default::default()
    };
    let mut process = PROCESS_INFORMATION::default();
    // SAFETY: The executable and mutable command line are NUL terminated. The
    // process/thread receive default security and no handles from this process.
    if unsafe {
        CreateProcessW(
            exe.as_ptr(),
            line.as_mut_ptr(),
            null(),
            null(),
            0,
            DETACHED_PROCESS,
            null(),
            null(),
            &startup,
            &mut process,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: CreateProcessW success transfers the two distinct live handles.
    unsafe {
        drop(OwnedHandle::from_raw_handle(process.hThread));
        Ok(GuiProcess(OwnedHandle::from_raw_handle(process.hProcess)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ptr::null_mut;
    use windows_sys::Win32::{
        Foundation::{ERROR_BROKEN_PIPE, HANDLE_FLAG_INHERIT, SetHandleInformation},
        Security::SECURITY_ATTRIBUTES,
        System::{
            Pipes::{CreatePipe, PeekNamedPipe},
            Threading::TerminateProcess,
        },
    };

    #[test]
    fn argument_keeps_spaces_quotes_unicode_and_trailing_backslash() {
        let mut line = Vec::new();
        for input in [r"C:\a b\velocity.exe", "", "a\\\"b", "測試\\"] {
            argument(&mut line, OsStr::new(input)).unwrap();
        }
        assert_eq!(
            String::from_utf16(&line).unwrap(),
            r#""C:\a b\velocity.exe" "" "a\\\"b" "測試\\""#
        );
    }

    #[test]
    #[ignore = "child process used by the handle inheritance test"]
    fn wait_for_probe_parent() {
        std::thread::sleep(std::time::Duration::from_secs(15));
    }

    #[test]
    fn detached_child_cannot_keep_a_redirected_pipe_open() {
        let attributes = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            bInheritHandle: 1,
            ..Default::default()
        };
        let (mut read, mut write) = (null_mut(), null_mut());
        // SAFETY: The arguments and two output slots live through CreatePipe.
        let created = unsafe { CreatePipe(&mut read, &mut write, &attributes, 0) };
        assert_ne!(created, 0);
        // SAFETY: CreatePipe succeeded and returned two owned handles.
        let (read, write) = unsafe {
            (
                OwnedHandle::from_raw_handle(read),
                OwnedHandle::from_raw_handle(write),
            )
        };
        // SAFETY: The pipe's read end does not need to be inheritable.
        let private = unsafe { SetHandleInformation(read.as_raw_handle(), HANDLE_FLAG_INHERIT, 0) };
        assert_ne!(private, 0);
        let mut command = Command::new(std::env::current_exe().unwrap());
        command.args([
            "--ignored",
            "--exact",
            "windows_gui_spawn::tests::wait_for_probe_parent",
        ]);
        let child = spawn(&command).unwrap();
        drop(write);
        // SAFETY: Zero-length peek; all optional output pointers are null.
        let peek = unsafe {
            PeekNamedPipe(
                read.as_raw_handle(),
                null_mut(),
                0,
                null_mut(),
                null_mut(),
                null_mut(),
            )
        };
        let error = io::Error::last_os_error();
        let alive = child.try_wait();
        // SAFETY: Terminate only the owned fixture child, including on test failure.
        unsafe {
            TerminateProcess(child.0.as_raw_handle(), 0);
            WaitForSingleObject(child.0.as_raw_handle(), 5000);
        }
        assert!(
            alive.unwrap().is_none(),
            "fixture exited before probing inheritance"
        );
        assert_eq!(peek, 0, "a live sibling kept the writer handle open");
        assert_eq!(error.raw_os_error(), Some(ERROR_BROKEN_PIPE as i32));
    }
}
