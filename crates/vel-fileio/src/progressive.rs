//! Progressive, normalized loading with streaming codec conversion and bounded snapshots.
use super::{DecodeOptions, DecodeReader, LineEnding, Loaded, Progress};
use std::{
    io::{self, BufReader, Read},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
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
    unchanged: Arc<AtomicBool>,
    binary: Arc<AtomicBool>,
}
impl LoadTask {
    /// A terminal decode result classified as binary by the shared policy.
    pub fn binary(&self) -> bool {
        self.binary.load(Ordering::Acquire)
    }
    /// Exact raw disk identity comparison, performed on the worker.
    /// Read this only after receiving the terminal result.
    pub fn unchanged(&self) -> bool {
        self.unchanged.load(Ordering::Acquire)
    }
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

/// Starts a progressive encoding-aware worker, retaining original-byte progress
/// and cancellation. Invalid codec attempts discard their provisional prefixes.
pub fn load_background(path: PathBuf, fallback: String) -> Result<LoadTask, String> {
    load_background_with_options(
        path,
        DecodeOptions {
            fallback_encoding: fallback,
            ..Default::default()
        },
    )
}
/// Loads on a cancellable worker, publishing shared prefixes for every supported codec.
pub fn load_background_with_options(
    path: PathBuf,
    options: DecodeOptions,
) -> Result<LoadTask, String> {
    start_background(path, options, false)
}
/// Loads an external revision, comparing raw identity off the UI thread.
/// Provisional prefixes never replace the live buffer during this comparison.
pub fn load_background_for_reload(
    path: PathBuf,
    options: DecodeOptions,
) -> Result<LoadTask, String> {
    start_background(path, options, true)
}
fn start_background(
    path: PathBuf,
    options: DecodeOptions,
    comparing: bool,
) -> Result<LoadTask, String> {
    load_on_workers(path, options, comparing, None)
}

/// Starts a progressive or comparing read on a bounded executor when configured.
pub fn load_on_workers(
    path: PathBuf,
    options: DecodeOptions,
    comparing: bool,
    workers: Option<(&crate::IoWorkers, usize)>,
) -> Result<LoadTask, String> {
    let binary = Arc::new(AtomicBool::new(false));
    let binary_result = binary.clone();
    let unchanged = Arc::new(AtomicBool::new(false));
    let comparison = unchanged.clone();
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
    let load = move || {
        if cancellation.is_cancelled() {
            return;
        }
        let publish = |snapshot| {
            if !comparing && !cancellation.is_cancelled() {
                let old = mailbox.lock().expect("prefix mailbox").replace(snapshot);
                drop(old);
            }
        };
        let mut loaded = (|| {
            let mut plan = super::decoding::DecodePlan::new(&path, &options)?;
            let mut failure = "invalid encoded text".to_owned();
            while let Some(attempt) = plan.next() {
                let file = super::regular::read(&path)?;
                let mut raw = super::identity::DigestReader::new(file);
                raw.read_exact(&mut vec![0; attempt.skip])
                    .map_err(|e| e.to_string())?;
                counter.store(attempt.skip as u64, Ordering::Relaxed);
                let mut emit = |mut snapshot: LoadSnapshot| {
                    snapshot.progress.read = counter.load(Ordering::Relaxed);
                    if !attempt.verify_legacy_revision {
                        publish(snapshot);
                    }
                };
                let (result, ending, line_counts) =
                    if attempt.encoding == encoding_rs::UTF_8 && !attempt.replacement {
                        // Keep first-snapshot latency and direct UTF-8 construction.
                        let mut normalized = Normalized::new(
                            &mut raw,
                            counter.clone(),
                            cancellation.clone(),
                            options.default_line_ending,
                        );
                        let result = stream_decoded_utf8(
                            &mut normalized,
                            Some(total),
                            &LoadOptions::default(),
                            &cancellation,
                            &mut emit,
                        );
                        (result, normalized.ending(), normalized.counts)
                    } else {
                        let mut progress = |p: Progress| {
                            counter.store(p.read + attempt.skip as u64, Ordering::Relaxed);
                        };
                        let mut decoder = DecodeReader::new(
                            CheckedReader {
                                input: &mut raw,
                                cancel: Some(&cancellation),
                            },
                            attempt.encoding,
                            total,
                            &mut progress,
                            options.default_line_ending,
                        );
                        decoder.allow_replacement = attempt.replacement;
                        let result = stream_decoded_utf8(
                            &mut decoder,
                            Some(total),
                            &LoadOptions::default(),
                            &cancellation,
                            &mut emit,
                        );
                        (result, decoder.ending(), decoder.counts)
                    };
                match result {
                    // Otherwise the file changed after the decision and the plan restarts.
                    Ok(text) if plan.read_matches_decision(&options, &raw.prefix)? => {
                        let loaded = super::finish_decoding(
                            &path,
                            &options,
                            plan.facts.clone(),
                            &attempt,
                            super::DecodedContent {
                                text,
                                ending,
                                line_counts,
                                content: raw.digest.identity(),
                            },
                        )?;
                        if plan.accept(&options, &attempt, &loaded)? {
                            return Ok(loaded);
                        }
                    }
                    Ok(_) => {}
                    Err(LoadError::DecodeRequired) => {}
                    Err(LoadError::Io(error)) if error.kind() == io::ErrorKind::InvalidData => {
                        failure = error.to_string();
                    }
                    Err(error) => return Err(error.to_string()),
                }
                // A failed strict attempt invalidates its read-only prefix.
                counter.store(0, Ordering::Relaxed);
                publish(LoadSnapshot {
                    text: vel_text::Text::new(),
                    progress: vel_largefile::Progress {
                        read: 0,
                        total: Some(total),
                    },
                    metrics: Default::default(),
                });
            }
            binary_result.store(plan.binary().is_some(), Ordering::Release);
            Err(plan
                .binary()
                .map_or(failure, |notice| notice.message(&path)))
        })();
        if let Ok(loaded) = &mut loaded {
            comparison.store(loaded.unchanged, Ordering::Release);
            if comparing && loaded.unchanged {
                // Retire the duplicate tree on this worker, not on the UI frame.
                loaded.text = vel_text::Text::new();
            }
        }
        let _ = tx.send(loaded.map_err(|error| super::regular::error(&path, error)));
    };
    if let Some((workers, limit)) = workers {
        workers.submit(limit, "velocity-file-load", load)?;
    } else {
        std::thread::Builder::new()
            .name("velocity-file-load".into())
            .spawn(load)
            .map_err(|e| e.to_string())?;
    }
    Ok(LoadTask {
        read,
        total,
        result,
        latest,
        cancel,
        unchanged,
        binary,
    })
}

// The source codec, binary decision and BOM consumption are already settled
// by decide_decoding. Feed normalized UTF-8 directly to the same rope snapshot
// builder as vel-largefile, without sniffing/stripping the converted text again.
fn stream_decoded_utf8(
    reader: impl Read,
    total: Option<u64>,
    options: &LoadOptions,
    cancel: &Cancellation,
    mut publish: impl FnMut(LoadSnapshot),
) -> Result<vel_text::Text, LoadError> {
    let mut metrics = vel_largefile::DocumentMetrics {
        lines: 1,
        ..Default::default()
    };
    let mut measured = 0;
    let mut line_bytes = 0;
    let result = vel_text::Text::from_reader_snapshots(
        reader,
        options.publish_bytes,
        options.publish_interval,
        |text| {
            if cancel.is_cancelled() {
                return Err(io::Error::other("load cancelled"));
            }
            for chunk in text.slice(measured..text.len_bytes()).chunks() {
                let mut start = 0;
                for lf in memchr::memchr_iter(b'\n', chunk.as_bytes()) {
                    line_bytes += (lf - start) as u64;
                    metrics.longest_line_bytes = metrics.longest_line_bytes.max(line_bytes);
                    line_bytes = 0;
                    start = lf + 1;
                }
                line_bytes += (chunk.len() - start) as u64;
                metrics.longest_line_bytes = metrics.longest_line_bytes.max(line_bytes);
            }
            measured = text.len_bytes();
            metrics.bytes = measured as u64;
            metrics.lines = text.len_lines() as u64;
            publish(LoadSnapshot {
                text,
                progress: vel_largefile::Progress { read: 0, total },
                metrics,
            });
            Ok(())
        },
    );
    match result {
        _ if cancel.is_cancelled() => Err(LoadError::Cancelled),
        Err(error) if error.kind() == io::ErrorKind::InvalidData => Err(LoadError::DecodeRequired),
        other => other.map_err(LoadError::Io),
    }
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
    default_line_ending: LineEnding,
    read: Arc<AtomicU64>,
    cancel: Cancellation,
}
impl<R: Read> Normalized<R> {
    fn new(
        input: R,
        read: Arc<AtomicU64>,
        cancel: Cancellation,
        default_line_ending: LineEnding,
    ) -> Self {
        Self {
            input: BufReader::with_capacity(128 * 1024, input),
            ready: Vec::new(),
            cursor: 0,
            cr: false,
            eof: false,
            counts: [0; 3],
            default_line_ending,
            read,
            cancel,
        }
    }
    fn ending(&self) -> LineEnding {
        if self.counts == [0; 3] {
            self.default_line_ending
        } else if self.counts[1] > self.counts[0] && self.counts[1] >= self.counts[2] {
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
