//! Headless kernel process clocks, resident size and physical footprint.
//! Adapted from Velocity’s MIT OR Apache-2.0 macOS P0 prototype.
use std::time::SystemTime;
#[cfg(target_os = "macos")]
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
    #[cfg(not(target_os = "macos"))]
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
    #[cfg(not(target_os = "macos"))]
    {
        Err("RSS measurement requires macOS task_info".into())
    }
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
        Err("physical footprint measurement requires macOS task_info".into())
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
    use super::*;
    #[test]
    fn kernel_readings_are_plausible_without_gpu() {
        #[cfg(target_os = "macos")]
        {
            let birth = process_birth().unwrap();
            let age = SystemTime::now().duration_since(birth).unwrap();
            assert!(age < Duration::from_secs(3600));
            assert!(rss_bytes().unwrap() > 0);
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
