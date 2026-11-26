//! Minimal libdispatch bindings for the main-queue memory pressure source.
use crate::{MemoryPressure, macos};
use std::{ffi::c_void, ptr::NonNull};

const WARN: libc::c_ulong = 0x02;
const CRITICAL: libc::c_ulong = 0x04;

unsafe extern "C" {
    // Only their addresses are used, as in the SDK's C macros.
    static _dispatch_main_q: u8;
    static _dispatch_source_type_memorypressure: u8;
    fn dispatch_source_create(
        kind: *const c_void,
        handle: libc::uintptr_t,
        mask: libc::c_ulong,
        queue: *const c_void,
    ) -> *mut c_void;
    fn dispatch_set_context(object: *mut c_void, context: *mut c_void);
    fn dispatch_source_set_event_handler_f(
        source: *mut c_void,
        handler: extern "C" fn(*mut c_void),
    );
    fn dispatch_source_get_data(source: *mut c_void) -> libc::c_ulong;
    fn dispatch_activate(object: *mut c_void);
    fn dispatch_source_cancel(source: *mut c_void);
    fn dispatch_release(object: *mut c_void);
}

pub(crate) struct PressureSource(NonNull<c_void>);

impl PressureSource {
    pub(crate) fn new() -> Option<Self> {
        // SAFETY: these are the SDK source type and queue addresses. Memory
        // pressure uses no handle. The created source is owned and inactive.
        let source = NonNull::new(unsafe {
            dispatch_source_create(
                (&raw const _dispatch_source_type_memorypressure).cast(),
                0,
                WARN | CRITICAL,
                (&raw const _dispatch_main_q).cast(),
            )
        })?;
        // SAFETY: libdispatch keeps the source live for the duration of its
        // own handler. The context is just a pointer to that source, not a retain.
        unsafe {
            dispatch_set_context(source.as_ptr(), source.as_ptr());
            dispatch_source_set_event_handler_f(source.as_ptr(), handle_event);
            dispatch_activate(source.as_ptr());
        }
        Some(Self(source))
    }
}

extern "C" fn handle_event(source: *mut c_void) {
    // SAFETY: the installed context is this still-live source.
    let flags = unsafe { dispatch_source_get_data(source) };
    if flags & CRITICAL != 0 {
        macos::queue_memory_pressure(MemoryPressure::Critical);
    } else if flags & WARN != 0 {
        macos::queue_memory_pressure(MemoryPressure::Warning);
    }
}

impl Drop for PressureSource {
    fn drop(&mut self) {
        // SAFETY: balanced with create/activate; cancellation stops delivery
        // and libdispatch retains any outstanding internal references.
        unsafe {
            dispatch_source_cancel(self.0.as_ptr());
            dispatch_release(self.0.as_ptr());
        }
    }
}

#[cfg(test)]
#[test]
fn memory_pressure_source_can_be_created_and_cancelled_without_appkit() {
    drop(PressureSource::new().expect("dispatch memory pressure source"));
}

#[cfg(test)]
#[test]
fn memory_footprint_tracks_touched_pages() {
    const MIB: u64 = 1 << 20;
    let before = crate::Platform::memory_footprint().expect("physical footprint");
    assert!(before > 0);
    // Anonymous VM guarantees fresh pages, independently of other tests' malloc caches.
    let size = (64 * MIB) as usize;
    let after = unsafe {
        // SAFETY: map fresh read/write memory, write within its bounds and unmap it once.
        let pages = libc::mmap(
            std::ptr::null_mut(),
            size,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_PRIVATE | libc::MAP_ANON,
            -1,
            0,
        );
        assert_ne!(pages, libc::MAP_FAILED);
        let page_size = libc::sysconf(libc::_SC_PAGESIZE);
        assert!(page_size > 0);
        for offset in (0..size).step_by(page_size as usize) {
            pages.cast::<u8>().add(offset).write_volatile(1);
        }
        let result = crate::Platform::memory_footprint();
        assert_eq!(libc::munmap(pages, size), 0);
        result.expect("footprint after touching pages")
    };
    eprintln!("touched 64 MiB: footprint {before} -> {after} bytes");
    assert!(after >= before + 32 * MIB, "{before} -> {after}");
}
