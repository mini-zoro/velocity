//! Headless kernel process clocks, resident size and physical footprint.
//! Adapted from Velocity’s MIT OR Apache-2.0 macOS P0 prototype.
use std::time::SystemTime;
#[cfg(any(target_os = "macos", windows))]
use std::time::{Duration, UNIX_EPOCH};

/// Read the kernel's process birth time, including loader work before Rust main.
pub fn process_birth() -> Result<SystemTime, String> {
    #[cfg(target_os = "macos")]
    {
        let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::uninit();
        let size = std::mem::size_of::<libc::proc_bsdinfo>();
        // SAFETY: the kernel writes at most `size` bytes into this valid output allocation.
        let written = unsafe {
            libc::proc_pidinfo(
                std::process::id() as i32,
                libc::PROC_PIDTBSDINFO,
                0,
                info.as_mut_ptr().cast(),
                size as i32,
            )
        };
        if written != size as i32 {
            return Err(format!(
                "proc_pidinfo returned {written}: {}",
                std::io::Error::last_os_error()
            ));
        }
        // SAFETY: the successful call above initialized the entire proc_bsdinfo structure.
        let info = unsafe { info.assume_init() };
        Ok(UNIX_EPOCH
            + Duration::from_secs(info.pbi_start_tvsec)
            + Duration::from_micros(info.pbi_start_tvusec))
    }
    #[cfg(windows)]
    {
        const EPOCH_DIFFERENCE: u64 = 116_444_736_000_000_000; // 1601 to 1970, in 100 ns ticks.
        let (birth, _) = win_process_times()?;
        let ticks = birth
            .checked_sub(EPOCH_DIFFERENCE)
            .ok_or("invalid process birth time")?;
        Ok(UNIX_EPOCH + Duration::from_nanos(ticks * 100))
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        Err("process birth measurement requires macOS".into())
    }
}
/// Current resident size, not the high-water RSS returned by getrusage.
pub fn rss_bytes() -> Result<u64, String> {
    #[cfg(target_os = "macos")]
    {
        let mut info = std::mem::MaybeUninit::<libc::mach_task_basic_info>::uninit();
        let mut count = libc::MACH_TASK_BASIC_INFO_COUNT;
        // SAFETY: task_info receives a valid task port and a correctly sized writable buffer/count.
        // libc still exposes the native ABI; avoid a second Mach binding in this small prototype.
        #[allow(deprecated)]
        let result = unsafe {
            libc::task_info(
                libc::mach_task_self(),
                libc::MACH_TASK_BASIC_INFO,
                info.as_mut_ptr().cast(),
                &mut count,
            )
        };
        if result != libc::KERN_SUCCESS || count != libc::MACH_TASK_BASIC_INFO_COUNT {
            return Err(format!("task_info code={result} count={count}"));
        }
        // SAFETY: KERN_SUCCESS and the checked count confirm the structure is initialized.
        Ok(unsafe { info.assume_init() }.resident_size)
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::{
            ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS},
            Threading::GetCurrentProcess,
        };
        let mut counters = PROCESS_MEMORY_COUNTERS::default();
        let size = size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
        counters.cb = size;
        // SAFETY: the pseudo-handle and the initialized output buffer are valid for this call.
        if unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, size) } == 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        Ok(counters.WorkingSetSize as u64)
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        Err("RSS measurement requires macOS task_info".into())
    }
}

#[cfg(windows)]
fn win_process_times() -> Result<(u64, u64), String> {
    use windows_sys::Win32::{
        Foundation::FILETIME,
        System::Threading::{GetCurrentProcess, GetProcessTimes},
    };
    let [mut created, mut exited, mut kernel, mut user] = [FILETIME::default(); 4];
    // SAFETY: all four outputs point to distinct live FILETIME structs.
    if unsafe {
        GetProcessTimes(
            GetCurrentProcess(),
            &mut created,
            &mut exited,
            &mut kernel,
            &mut user,
        )
    } == 0
    {
        return Err(std::io::Error::last_os_error().to_string());
    }
    let ticks = |t: FILETIME| (u64::from(t.dwHighDateTime) << 32) | u64::from(t.dwLowDateTime);
    Ok((ticks(created), ticks(kernel) + ticks(user)))
}

/// Total user and kernel CPU time for the Windows process.
#[cfg(windows)]
pub fn process_cpu_ms() -> Result<f64, String> {
    Ok(win_process_times()?.1 as f64 / 10_000.)
}

/// Total user and kernel CPU time for the calling Windows thread.
#[cfg(windows)]
pub fn thread_cpu() -> Result<Duration, String> {
    use windows_sys::Win32::{
        Foundation::FILETIME,
        System::Threading::{GetCurrentThread, GetThreadTimes},
    };
    let [mut created, mut exited, mut kernel, mut user] = [FILETIME::default(); 4];
    // SAFETY: all four outputs point to distinct live FILETIME structs.
    if unsafe {
        GetThreadTimes(
            GetCurrentThread(),
            &mut created,
            &mut exited,
            &mut kernel,
            &mut user,
        )
    } == 0
    {
        return Err(std::io::Error::last_os_error().to_string());
    }
    let ticks = |t: FILETIME| (u64::from(t.dwHighDateTime) << 32) | u64::from(t.dwLowDateTime);
    Ok(Duration::from_nanos((ticks(kernel) + ticks(user)) * 100))
}

// Stable TASK_VM_INFO revision 1 prefix from the macOS SDK's mach/task_info.h.
// libc exposes task_info but not this flavor's payload. Request only the prefix
// through phys_footprint so newer trailing fields and kernel revisions are irrelevant.
#[cfg(target_os = "macos")]
#[repr(C)]
#[derive(Debug, Default)]
struct TaskVmInfoRev1 {
    virtual_size: u64,
    region_count: i32,
    page_size: i32,
    resident_size: u64,
    resident_size_peak: u64,
    device: u64,
    device_peak: u64,
    internal: u64,
    internal_peak: u64,
    external: u64,
    external_peak: u64,
    reusable: u64,
    reusable_peak: u64,
    purgeable_volatile_pmap: u64,
    purgeable_volatile_resident: u64,
    purgeable_volatile_virtual: u64,
    compressed: u64,
    compressed_peak: u64,
    compressed_lifetime: u64,
    phys_footprint: u64,
}

#[cfg(target_os = "macos")]
const TASK_VM_INFO: libc::task_flavor_t = 22;
#[cfg(target_os = "macos")]
const TASK_VM_INFO_REV1_COUNT: libc::mach_msg_type_number_t =
    (std::mem::size_of::<TaskVmInfoRev1>() / std::mem::size_of::<libc::natural_t>()) as u32;

/// Current task memory charge, using the footprint accounting shown by Activity Monitor.
/// This differs from RSS and includes charges such as compressed private memory.
pub fn phys_footprint_bytes() -> Result<u64, String> {
    #[cfg(target_os = "macos")]
    {
        let mut info = TaskVmInfoRev1::default();
        let mut count = TASK_VM_INFO_REV1_COUNT;
        // SAFETY: the repr(C) revision-1 prefix matches the SDK ABI. count is in
        // natural_t words and describes exactly this initialized, writable buffer.
        #[allow(deprecated)]
        let result = unsafe {
            libc::task_info(
                libc::mach_task_self(),
                TASK_VM_INFO,
                (&mut info as *mut TaskVmInfoRev1).cast(),
                &mut count,
            )
        };
        checked_footprint(result, count, &info)
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err("macOS physical footprint accounting is unavailable on this platform".into())
    }
}

#[cfg(target_os = "macos")]
fn checked_footprint(
    result: libc::kern_return_t,
    count: libc::mach_msg_type_number_t,
    info: &TaskVmInfoRev1,
) -> Result<u64, String> {
    if result != libc::KERN_SUCCESS || count != TASK_VM_INFO_REV1_COUNT {
        return Err(format!(
            "task_info(TASK_VM_INFO) code={result} count={count}, expected revision-1 count={TASK_VM_INFO_REV1_COUNT}"
        ));
    }
    Ok(info.phys_footprint)
}

#[cfg(test)]
mod tests {
    #[cfg(any(target_os = "macos", windows))]
    use super::*;
    #[test]
    fn kernel_readings_are_plausible_without_gpu() {
        #[cfg(any(target_os = "macos", windows))]
        {
            let birth = process_birth().unwrap();
            let age = SystemTime::now().duration_since(birth).unwrap();
            assert!(age < Duration::from_secs(3600));
            assert!(rss_bytes().unwrap() > 0);
            #[cfg(target_os = "macos")]
            assert!(phys_footprint_bytes().unwrap() > 0);
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn vm_info_prefix_matches_sdk_and_rejects_short_or_failed_replies() {
        assert_eq!(TASK_VM_INFO, 22);
        assert_eq!(std::mem::offset_of!(TaskVmInfoRev1, phys_footprint), 144);
        assert_eq!(std::mem::size_of::<TaskVmInfoRev1>(), 152);
        assert_eq!(TASK_VM_INFO_REV1_COUNT, 38);
        let info = TaskVmInfoRev1 {
            resident_size: 123,
            phys_footprint: 9_000_000_000,
            ..Default::default()
        };
        assert_eq!(
            checked_footprint(libc::KERN_SUCCESS, 38, &info),
            Ok(9_000_000_000)
        );
        assert!(checked_footprint(libc::KERN_SUCCESS, 36, &info).is_err());
        assert!(checked_footprint(libc::KERN_FAILURE, 38, &info).is_err());
    }
}
