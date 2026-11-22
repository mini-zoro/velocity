//! Read-only hex tabs backed by an immutable, application-owned snapshot.
use std::{
    ops::Range,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
};
use vel_largefile::HexView;

#[cfg(windows)]
pub(crate) fn make_sparse(file: &std::fs::File) -> bool {
    use std::{os::windows::io::AsRawHandle, ptr};
    use windows_sys::Win32::System::{IO::DeviceIoControl, Ioctl::FSCTL_SET_SPARSE};

    let mut returned = 0;
    // SAFETY: file is open for synchronous writes and neither buffer is needed.
    unsafe {
        DeviceIoControl(
            file.as_raw_handle(),
            FSCTL_SET_SPARSE,
            ptr::null(),
            0,
            ptr::null_mut(),
            0,
            &mut returned,
            ptr::null_mut(),
        ) != 0
    }
}

#[derive(Debug)]
pub struct Snapshot {
    pub view: HexView,
    // Kept after the map so the mapping is dropped before its backing directory.
    _directory: tempfile::TempDir,
}
#[derive(Debug)]
pub struct Task {
    pub result: mpsc::Receiver<Result<Arc<Snapshot>, String>>,
    cancel: Arc<AtomicBool>,
    pub total: u64,
}
impl Drop for Task {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
    }
}
impl Task {
    pub fn open(path: PathBuf) -> Result<Self, String> {
        let total = vel_fileio::regular::metadata(&path, false)?
            .ok_or("missing document")?
            .len();
        let cancel = Arc::new(AtomicBool::new(false));
        let cancelled = cancel.clone();
        let (tx, result) = mpsc::channel();
        std::thread::Builder::new()
            .name("velocity-hex-snapshot".into())
            .spawn(move || {
                let result = (|| {
                    let directory = tempfile::tempdir().map_err(|e| e.to_string())?;
                    let copy = directory.path().join("snapshot");
                    // Copy only from the checked descriptor, with cancellation between chunks.
                    // No map of a mutable source or unchecked path-based copy exists.
                    let mut source = vel_fileio::regular::read(&path)?;
                    let mut output = std::fs::File::create(&copy).map_err(|e| e.to_string())?;
                    use std::io::{Read, Write};
                    #[cfg(windows)]
                    use std::io::{Seek, SeekFrom};
                    #[cfg(windows)]
                    let mut bytes = vec![0; total.clamp(1, 1024 * 1024) as usize];
                    #[cfg(windows)]
                    let sparse = make_sparse(&output);
                    #[cfg(windows)]
                    let mut skipped = 0;
                    #[cfg(not(windows))]
                    let mut bytes = [0; 64 * 1024];
                    loop {
                        if cancelled.load(Ordering::Acquire) {
                            return Err("hex load cancelled".into());
                        }
                        let n = source
                            .read(&mut bytes)
                            .map_err(|e| vel_fileio::regular::error(&path, e))?;
                        if n == 0 {
                            break;
                        }
                        #[cfg(windows)]
                        if sparse {
                            if bytes[..n].iter().all(|&byte| byte == 0) {
                                skipped += n as i64;
                                continue;
                            }
                            if skipped != 0 {
                                output
                                    .seek(SeekFrom::Current(skipped))
                                    .map_err(|e| e.to_string())?;
                                skipped = 0;
                            }
                        }
                        output.write_all(&bytes[..n]).map_err(|e| e.to_string())?;
                    }
                    if cancelled.load(Ordering::Acquire) {
                        return Err("hex load cancelled".into());
                    }
                    #[cfg(windows)]
                    if skipped != 0 {
                        let end = output
                            .seek(SeekFrom::Current(skipped))
                            .map_err(|e| e.to_string())?;
                        output.set_len(end).map_err(|e| e.to_string())?;
                    }
                    drop(output);
                    // SAFETY: this private directory and file are owned solely by Snapshot. No
                    // writer or writable handle escapes, and the map drops before the directory.
                    let view = unsafe { HexView::open(&copy) }.map_err(|e| e.to_string())?;
                    Ok(Arc::new(Snapshot {
                        view,
                        _directory: directory,
                    }))
                })();
                let _ = tx.send(result);
            })
            .map_err(|e| e.to_string())?;
        Ok(Self {
            result,
            cancel,
            total,
        })
    }
}
pub fn copy(snapshot: &Snapshot, range: Range<usize>, hex: bool) -> String {
    let end = range.end.min(snapshot.view.len_bytes() as usize);
    let start = range.start.min(end);
    let mut bytes = Vec::with_capacity(if hex { 0 } else { end - start });
    let mut formatted = String::with_capacity(if hex {
        (end - start).saturating_mul(3).saturating_sub(1)
    } else {
        0
    });
    for row in start / 16..end.div_ceil(16) {
        if let Some(r) = snapshot.view.row(row as u64) {
            let a = start.saturating_sub(row * 16);
            let b = end.saturating_sub(row * 16).min(r.bytes.len());
            if hex {
                use std::fmt::Write;
                for byte in &r.bytes[a..b] {
                    if !formatted.is_empty() {
                        formatted.push(' ');
                    }
                    write!(formatted, "{byte:02x}").expect("String write");
                }
            } else {
                bytes.extend_from_slice(&r.bytes[a..b]);
            }
        }
    }
    if hex {
        formatted
    } else {
        String::from_utf8_lossy(&bytes).into_owned()
    }
}

pub fn offset(value: &str) -> Result<usize, String> {
    let value = value.trim();
    if let Some(hex) = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
    {
        usize::from_str_radix(hex, 16)
    } else {
        value.parse()
    }
    .map_err(|_| "Offset must be decimal or 0x-prefixed hexadecimal".into())
}
