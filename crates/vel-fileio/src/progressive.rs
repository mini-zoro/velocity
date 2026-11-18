//! Progressive, normalized UTF-8 loading; encoding fallback remains a worker-only full decode.
use super::{Format, LineEnding, Loaded, Progress, load_cancellable};
use std::{
    io::{self, BufReader, Read},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
        mpsc::{self, Receiver},
    },
};
use vel_largefile::{Cancellation, ContentKind, LoadError, LoadOptions, LoadSnapshot};

thread_local! {
    static READ_CANCEL: std::cell::RefCell<Option<Cancellation>> = const { std::cell::RefCell::new(None) };
}
/// Applies cooperative cancellation to synchronous document loads on this worker.
/// Restore histories use the same decoder as ordinary file loads.
pub fn with_cancellation<T>(cancel: Cancellation, read: impl FnOnce() -> T) -> T {
    struct Reset(Option<Cancellation>);
    impl Drop for Reset {
        fn drop(&mut self) {
            READ_CANCEL.with(|c| c.replace(self.0.take()));
        }
    }
    let _reset = Reset(READ_CANCEL.with(|c| c.replace(Some(cancel))));
    read()
}
pub(super) fn current_cancellation() -> Option<Cancellation> {
    READ_CANCEL.with(|c| c.borrow().clone())
}

/// Samples the same first 8 KiB/BOM heuristic as the large-file engine.
pub fn content_kind(path: &Path) -> Result<ContentKind, String> {
    let mut file = super::regular::read(path)?;
    let mut bytes = Vec::with_capacity(8192);
    file.by_ref()
        .take(8192)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(vel_largefile::detect_content(&bytes))
}

/// One cancellable worker, one coalescing prefix slot and an independent terminal result.
#[derive(Debug)]
pub struct LoadTask {
    /// Original bytes read, including the BOM (reset on encoding fallback).
    pub read: Arc<AtomicU64>,
    /// Original file size; EOF still determines the actual size.
    pub total: u64,
    /// Final decoded, normalized text and source format.
    pub result: Receiver<Result<Loaded, String>>,
    latest: Arc<Mutex<Option<LoadSnapshot>>>,
    cancel: Cancellation,
}
impl LoadTask {
    /// Poll before the terminal result. An empty prefix invalidates a failed UTF-8 attempt.
    pub fn try_snapshot(&self) -> Option<LoadSnapshot> {
        self.latest.try_lock().ok()?.take()
    }
    /// Requests cancellation without joining or blocking the caller.
    pub fn cancel(&self) {
        self.cancel.cancel();
    }
    /// Shared cancellation handle, also useful for lifecycle diagnostics.
    pub fn cancellation(&self) -> Cancellation {
        self.cancel.clone()
    }
}
impl Drop for LoadTask {
    fn drop(&mut self) {
        self.cancel();
    }
}

/// Starts a progressive UTF-8 worker. Other encodings discard prefixes and fully decode
/// on this same worker, retaining original-byte progress and cancellation.
pub fn load_background(path: PathBuf, fallback: String) -> Result<LoadTask, String> {
    let total = super::regular::metadata(&path, false)?
        .ok_or("missing document")?
        .len();
    let read = Arc::new(AtomicU64::new(0));
    let counter = read.clone();
    let latest = Arc::new(Mutex::new(None));
    let mailbox = latest.clone();
    let cancel = Cancellation::default();
    let cancellation = cancel.clone();
    let (tx, result) = mpsc::channel();
    std::thread::Builder::new()
        .name("velocity-file-load".into())
        .spawn(move || {
            let publish = |snapshot| {
                if !cancellation.is_cancelled() {
                    let old = mailbox.lock().expect("prefix mailbox").replace(snapshot);
                    drop(old);
                }
            };
            let loaded = (|| {
                let kind = content_kind(&path)?;
                if kind == ContentKind::Binary {
                    return Err("binary file: use hexadecimal view".into());
                }
                if kind == ContentKind::Utf8 {
                    let file = super::regular::read(&path)?;
                    let mut normalized =
                        Normalized::new(file, counter.clone(), cancellation.clone());
                    match vel_largefile::stream_utf8(
                        &mut normalized,
                        Some(total),
                        &LoadOptions::default(),
                        &cancellation,
                        |mut s| {
                            s.progress.read = counter.load(Ordering::Relaxed);
                            publish(s);
                        },
                    ) {
                        Ok(loaded) => {
                            return Ok(Loaded {
                                text: loaded.text,
                                format: Format {
                                    encoding: "UTF-8".into(),
                                    bom: loaded.bom,
                                    line_ending: normalized.ending(),
                                },
                                source_bytes: counter.load(Ordering::Relaxed),
                            });
                        }
                        Err(LoadError::DecodeRequired) => {}
                        Err(e) => return Err(e.to_string()),
                    }
                }
                // The old root is never editable/savable. Invalidate it before retrying the codec.
                counter.store(0, Ordering::Relaxed);
                publish(LoadSnapshot {
                    text: vel_text::Text::new(),
                    progress: vel_largefile::Progress {
                        read: 0,
                        total: Some(total),
                    },
                    metrics: vel_largefile::DocumentMetrics::default(),
                });
                load_cancellable(
                    &path,
                    &fallback,
                    &mut |p: Progress| {
                        counter.store(p.read, Ordering::Relaxed);
                    },
                    Some(&cancellation),
                )
            })();
            let _ = tx.send(loaded.map_err(|e| super::regular::error(&path, e)));
        })
        .map_err(|e| e.to_string())?;
    Ok(LoadTask {
        read,
        total,
        result,
        latest,
        cancel,
    })
}

pub(super) struct CheckedReader<'a, R> {
    pub input: R,
    pub cancel: Option<&'a Cancellation>,
}
impl<R: Read> Read for CheckedReader<'_, R> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        if self.cancel.is_some_and(Cancellation::is_cancelled) {
            return Err(io::Error::other("load cancelled"));
        }
        self.input.read(bytes)
    }
}

struct Normalized<R> {
    input: BufReader<R>,
    ready: Vec<u8>,
    cursor: usize,
    cr: bool,
    eof: bool,
    counts: [u64; 3],
    read: Arc<AtomicU64>,
    cancel: Cancellation,
}
impl<R: Read> Normalized<R> {
    fn new(input: R, read: Arc<AtomicU64>, cancel: Cancellation) -> Self {
        Self {
            input: BufReader::with_capacity(128 * 1024, input),
            ready: Vec::new(),
            cursor: 0,
            cr: false,
            eof: false,
            counts: [0; 3],
            read,
            cancel,
        }
    }
    fn ending(&self) -> LineEnding {
        if self.counts[1] > self.counts[0] && self.counts[1] >= self.counts[2] {
            LineEnding::CrLf
        } else if self.counts[2] > self.counts[0] && self.counts[2] > self.counts[1] {
            LineEnding::Cr
        } else {
            LineEnding::Lf
        }
    }
}
impl<R: Read> Read for Normalized<R> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if out.is_empty() {
            return Ok(0);
        }
        if self.cancel.is_cancelled() {
            return Err(io::Error::other("load cancelled"));
        }
        while self.cursor == self.ready.len() && !self.eof {
            self.ready.clear();
            self.cursor = 0;
            let mut raw = [0; 128 * 1024];
            let n = self.input.read(&mut raw)?;
            self.read.fetch_add(n as u64, Ordering::Relaxed);
            self.eof = n == 0;
            for &byte in &raw[..n] {
                if self.cr {
                    self.ready.push(b'\n');
                    self.cr = false;
                    if byte == b'\n' {
                        self.counts[1] += 1;
                        continue;
                    }
                    self.counts[2] += 1;
                }
                if byte == b'\r' {
                    self.cr = true;
                } else {
                    self.ready.push(byte);
                    if byte == b'\n' {
                        self.counts[0] += 1;
                    }
                }
            }
            if self.eof && self.cr {
                self.ready.push(b'\n');
                self.counts[2] += 1;
                self.cr = false;
            }
        }
        let n = out.len().min(self.ready.len() - self.cursor);
        out[..n].copy_from_slice(&self.ready[self.cursor..self.cursor + n]);
        self.cursor += n;
        Ok(n)
    }
}
