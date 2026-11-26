use crate::MemoryPressure;
use std::{
    cell::Cell,
    mem::size_of,
    ptr::NonNull,
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::{CloseHandle, HANDLE},
    System::{
        Memory::{
            CreateMemoryResourceNotification, GetProcessHeap, HeapCompact,
            LowMemoryResourceNotification, QueryMemoryResourceNotification,
        },
        ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS_EX},
        Threading::GetCurrentProcess,
    },
};

pub(crate) struct PressureSource {
    handle: HANDLE,
    next: Cell<Option<Instant>>,
    warned: Cell<bool>,
}

impl PressureSource {
    pub(crate) fn new() -> Option<Self> {
        // SAFETY: the returned non-null handle is owned and closed by Drop.
        let handle = NonNull::new(unsafe {
            CreateMemoryResourceNotification(LowMemoryResourceNotification)
        })?;
        Some(Self {
            handle: handle.as_ptr(),
            next: Cell::new(None),
            warned: Cell::new(false),
        })
    }

    pub(crate) fn poll(&self, now: Instant) -> Option<MemoryPressure> {
        if self.next.get().is_some_and(|next| now < next) {
            return None;
        }
        self.next.set(Some(now + Duration::from_secs(5)));
        let mut low = 0;
        // SAFETY: the handle is live, and low is a writable BOOL.
        if unsafe { QueryMemoryResourceNotification(self.handle, &mut low) } == 0 {
            return None;
        }
        // The Win32 notification is level-triggered and has no critical tier.
        let was_low = self.warned.replace(low != 0);
        (low != 0 && !was_low).then_some(MemoryPressure::Warning)
    }
}

impl Drop for PressureSource {
    fn drop(&mut self) {
        // SAFETY: this owns the successful CreateMemoryResourceNotification result.
        unsafe { CloseHandle(self.handle) };
    }
}

pub(crate) fn footprint() -> Option<u64> {
    let size = size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
    let mut counters = PROCESS_MEMORY_COUNTERS_EX {
        cb: size,
        ..Default::default()
    };
    // SAFETY: PSAPI uses cb to select the extended layout. The process pseudo-handle
    // can be queried here without opening or closing a handle.
    let ok = unsafe { GetProcessMemoryInfo(GetCurrentProcess(), (&raw mut counters).cast(), size) };
    (ok != 0).then_some(counters.PrivateUsage as u64)
}

pub(crate) fn compact_heap() {
    // SAFETY: the standard process heap remains live for the process lifetime;
    // flags=0 keeps the default serialization.
    unsafe {
        let heap = GetProcessHeap();
        if !heap.is_null() {
            HeapCompact(heap, 0);
        }
    }
}
