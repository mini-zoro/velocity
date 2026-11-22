//! The local endpoint name, shared with the standalone packaging fixture.
use std::path::{Path, PathBuf};

pub fn canonical_profile(path: &Path) -> PathBuf {
    // A fresh profile may not exist yet. Resolve the nearest existing ancestor
    // so creating the profile does not change its endpoint (including /tmp).
    path.canonicalize()
        .unwrap_or_else(|_| match (path.parent(), path.file_name()) {
            (Some(parent), Some(name)) => canonical_profile(parent).join(name),
            _ => path.to_owned(),
        })
}

#[cfg(unix)]
pub fn socket_path(data: Option<&Path>, uid: u32) -> PathBuf {
    // FNV-1a, fixed width and independent of Rust's randomized hashers.
    let mut hash = 0xcbf29ce484222325_u64;
    if let Some(data) = data {
        for &byte in canonical_profile(data).as_os_str().as_encoded_bytes() {
            hash = (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3);
        }
    }
    // Use the short fixed per-user base (not shell TMPDIR). Darwin unix(4)
    // reserves 104 bytes for sun_path, including the terminating NUL.
    // https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man4/unix.4.html
    PathBuf::from(format!("/tmp/velocity-{uid}/{hash:016x}.sock"))
}
