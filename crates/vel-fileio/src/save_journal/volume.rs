use std::fs::File;

#[derive(Default)]
pub(super) struct Volume {
    pub uuid: Option<[u8; 16]>,
    pub synthetic_empty_id: bool,
    pub coarse_timestamps: bool,
    #[cfg(target_os = "macos")]
    pub apfs: bool,
}

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
pub(super) fn inspect(file: &File) -> Volume {
    use std::{mem::MaybeUninit, os::fd::AsRawFd};

    let fd = file.as_raw_fd();
    let mut stat = MaybeUninit::<libc::statfs>::uninit();
    // SAFETY: fstatfs initializes the output on success; fd remains live.
    let stat = unsafe { (libc::fstatfs(fd, stat.as_mut_ptr()) == 0).then(|| stat.assume_init()) };
    let name = stat.as_ref().map_or(b"".as_slice(), |stat| {
        // SAFETY: Darwin supplies nul-terminated f_fstypename.
        let name = unsafe { std::ffi::CStr::from_ptr(stat.f_fstypename.as_ptr()) };
        name.to_bytes()
    });
    let synthetic_empty_id = matches!(name, b"msdos" | b"exfat");
    // HFS+ persists whole seconds; FAT-family timestamps also may not
    // distinguish consecutive same-size writes.
    // https://developer.apple.com/library/archive/technotes/tn/tn1150.html#HFSPlusDates
    // https://learn.microsoft.com/en-us/windows/win32/fileio/exfat-specification#748-timestamp-fields
    // ATTR_VOL_UUID identifies the volume independently of its mount's st_dev.
    // Darwin packs a u32 result length followed by the fixed 16-byte UUID.
    // https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/getattrlist.2.html
    // https://github.com/apple-oss-distributions/xnu/blob/main/bsd/sys/attr.h
    let mut attrs = libc::attrlist {
        bitmapcount: libc::ATTR_BIT_MAP_COUNT,
        reserved: 0,
        commonattr: 0,
        volattr: libc::ATTR_VOL_INFO | libc::ATTR_VOL_UUID,
        dirattr: 0,
        fileattr: 0,
        forkattr: 0,
    };
    #[repr(C)]
    struct Buffer {
        len: u32,
        uuid: [u8; 16],
    }
    let mut result = Buffer {
        len: 0,
        uuid: [0; 16],
    };
    let size = size_of::<Buffer>();
    // SAFETY: the bitmap and fixed-size result have the ABI layout described above.
    let mut status = unsafe {
        libc::fgetattrlist(
            fd,
            (&raw mut attrs).cast(),
            (&raw mut result).cast(),
            size,
            0,
        )
    };
    // Some drivers only expose volume attributes at the mount root.
    if (status != 0 || result.uuid == [0; 16])
        && let Some(stat) = &stat
    {
        use std::os::unix::{ffi::OsStrExt, fs::MetadataExt};
        // SAFETY: f_mntonname is a nul-terminated mount pathname from fstatfs.
        let name = unsafe { std::ffi::CStr::from_ptr(stat.f_mntonname.as_ptr()) };
        let path = std::path::Path::new(std::ffi::OsStr::from_bytes(name.to_bytes()));
        if file
            .metadata()
            .ok()
            .zip(path.metadata().ok())
            .is_some_and(|(file, root)| file.dev() == root.dev())
        {
            // SAFETY: same output layout, and name is a live mount pathname.
            status = unsafe {
                libc::getattrlist(
                    name.as_ptr(),
                    (&raw mut attrs).cast(),
                    (&raw mut result).cast(),
                    size,
                    0,
                )
            };
        }
    }
    let volume = Volume {
        uuid: (status == 0 && result.len as usize == size && result.uuid != [0; 16])
            .then_some(result.uuid),
        synthetic_empty_id,
        coarse_timestamps: synthetic_empty_id || name == b"hfs",
        apfs: name == b"apfs",
    };
    #[cfg(test)]
    return Volume {
        uuid: UUID_PROBE.get().or(volume.uuid),
        synthetic_empty_id: FAT_PROBE.get() || volume.synthetic_empty_id,
        coarse_timestamps: super::COARSE_VERSION_PROBE.get() || volume.coarse_timestamps,
        apfs: volume.apfs && !FAT_PROBE.get(),
    };
    #[cfg(not(test))]
    volume
}

#[cfg(not(target_os = "macos"))]
pub(super) fn inspect(_file: &File) -> Volume {
    Volume::default()
}

#[cfg(all(test, target_os = "macos"))]
thread_local! {
    pub(super) static FAT_PROBE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    pub(super) static UUID_PROBE: std::cell::Cell<Option<[u8; 16]>> = const { std::cell::Cell::new(None) };
}
