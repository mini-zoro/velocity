use std::{ffi::OsStr, os::windows::ffi::OsStrExt};
use windows_sys::Win32::Globalization::{CSTR_EQUAL, CompareStringOrdinal};

/// Compares component spellings using Win32 ordinal case folding and equivalent separators.
/// This is a lexical comparison; it does not resolve paths or query volume settings.
pub fn windows_path_component_eq(a: &OsStr, b: &OsStr) -> bool {
    fn wide(s: &OsStr) -> Vec<u16> {
        s.encode_wide()
            .map(|u| match u {
                0x2f => 0x5c, // Normalize '/' to '\\', including inside UNC prefixes.
                _ => u,
            })
            .collect()
    }
    let (a, b) = (wide(a), wide(b));
    let (Ok(an), Ok(bn)) = (i32::try_from(a.len()), i32::try_from(b.len())) else {
        return false;
    };
    // SAFETY: both buffers are live and the lengths count their UTF-16 code units.
    unsafe { CompareStringOrdinal(a.as_ptr(), an, b.as_ptr(), bn, 1) == CSTR_EQUAL }
}
