//! Windows console client setup. Native window startup belongs to vel-platform.
use std::io;
use windows_sys::Win32::System::{
    Console::{CTRL_C_EVENT, SetConsoleCtrlHandler},
    Threading::ExitProcess,
};

unsafe extern "system" fn interrupt_wait(event: u32) -> i32 {
    if event == CTRL_C_EVENT {
        // SAFETY: this runs only in the waiting CLI client, not in the primary.
        unsafe { ExitProcess(130) };
    }
    0
}

pub fn install_client_interrupt() -> io::Result<()> {
    // SAFETY: the handler has the Win32 console callback ABI and static lifetime.
    if unsafe { SetConsoleCtrlHandler(Some(interrupt_wait), 1) } == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}
