//! UTF-8 prefix snapshots with bounded staging and a coalescing worker mailbox.
use crate::{ContentKind, DocumentMetrics, detect_content};
use std::{
    cell::Cell,
    fs::File,
    io::{self, Cursor, Read},
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver},
    },
    thread,
    time::Duration,
};
use vel_text::Text;

/// Cooperative cancellation; dropping a task also sets this flag.
#[derive(Debug, Clone, Default)]
pub struct Cancellation(Arc<AtomicBool>);
impl Cancellation {
    /// Requests cancellation at the next read/build/publication boundary.
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }
    /// Whether cancellation has been requested.
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
    fn check(&self) -> Result<(), LoadError> {
        if self.is_cancelled() {
            Err(LoadError::Cancelled)
        } else {
            Ok(())
        }
    }
}
/// Bounded streaming and publication settings.
#[derive(Debug, Clone)]
pub struct LoadOptions {
    /// Maximum source read request, clamped to 4 bytes..=2 MiB. Default 2 MiB.
    /// The pipeline has at most four 2 MiB blocks in flight, independent of this hint.
    pub chunk_bytes: usize,
    /// Publish after this many additional decoded bytes. Default 16 MiB.
    pub publish_bytes: usize,
    /// Also publish after this interval, checked between chunks. Default 100 ms.
    pub publish_interval: Duration,
}
impl Default for LoadOptions {
    fn default() -> Self {
        Self {
            chunk_bytes: 2 * 1024 * 1024,
            publish_bytes: 16 * 1024 * 1024,
            publish_interval: Duration::from_millis(100),
        }
    }
}
/// Source byte progress. `read` includes the UTF-8 BOM and any incomplete scalar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Progress {
    /// Bytes consumed from the source.
    pub read: u64,
    /// File length, or None for streams of unknown length. It is a hint, not an EOF.
    pub total: Option<u64>,
}
/// A provisional, immutable loaded prefix. Discard it if the terminal result fails.
#[derive(Debug, Clone)]
pub struct LoadSnapshot {
    /// Shared rope root; published UTF-8 always ends at a complete scalar.
    pub text: Text,
    /// Progress in original disk bytes.
    pub progress: Progress,
    /// Prefix measurements, sufficient for conservative feature policy decisions.
    pub metrics: DocumentMetrics,
}
/// Complete UTF-8 load. Line endings are preserved verbatim, as in vel-text.
#[derive(Debug)]
pub struct Loaded {
    /// Final shared rope root.
    pub text: Text,
    /// Whether a UTF-8 BOM was stripped from the beginning.
    pub bom: bool,
    /// Original byte count and optional initial file-length hint.
    pub progress: Progress,
    /// Final decoded document measurements.
    pub metrics: DocumentMetrics,
}
/// Routing/error result. No encoding errors are silently replaced.
#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    /// A source read or worker-spawn failure.
    #[error("large-file I/O: {0}")]
    Io(#[from] io::Error),
    /// UTF-16 BOM or invalid/truncated UTF-8: discard prefixes and use vel-fileio.
    #[error("encoding-aware full decode required")]
    DecodeRequired,
    /// NUL in the first 8 KiB, except when a UTF-16 BOM establishes text.
    #[error("binary content: use a hex view")]
    Binary,
    /// Explicit cancellation or a dropped task.
    #[error("load cancelled")]
    Cancelled,
}
/// Streams UTF-8 on the calling thread. The reader need not be Send or static.
/// Publishes the first constructed 2 MiB block immediately, then at the byte/time cadence,
/// and at EOF (including empty files). Staging is at most 8 MiB; snapshots share trees.
///
/// A UTF-8 BOM is stripped. CR/CRLF are preserved, not normalized. Other encodings
/// return DecodeRequired; the caller must discard provisional prefixes and run its
/// full encoding-aware decoder. Cancellation cannot interrupt a blocking Read call.
pub fn stream_utf8(
    mut reader: impl Read,
    total: Option<u64>,
    options: &LoadOptions,
    cancel: &Cancellation,
    mut publish: impl FnMut(LoadSnapshot),
) -> Result<Loaded, LoadError> {
    cancel.check()?;
    let mut prefix = Vec::with_capacity(8192);
    // A bounded sniff handles BOMs split across arbitrarily short reads.
    while prefix.len() < 8192 {
        cancel.check()?;
        let mut sample = [0; 8192];
        match reader.read(&mut sample[..8192 - prefix.len()]) {
            Ok(0) => break,
            Ok(n) => prefix.extend_from_slice(&sample[..n]),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e.into()),
        }
    }
    match detect_content(&prefix) {
        ContentKind::Binary => return Err(LoadError::Binary),
        ContentKind::DecodeRequired => return Err(LoadError::DecodeRequired),
        ContentKind::Utf8 => {}
    }
    let bom = prefix.starts_with(&[0xef, 0xbb, 0xbf]);
    let skip = if bom { 3 } else { 0 };
    let read = Cell::new(skip as u64);
    let mut reader = CheckedReader {
        inner: Cursor::new(&prefix[skip..]).chain(reader),
        read: &read,
        cancel,
        max_read: options.chunk_bytes.clamp(4, 2 * 1024 * 1024),
    };
    let mut metrics = DocumentMetrics {
        lines: 1,
        ..DocumentMetrics::default()
    };
    let mut line_bytes = 0u64;
    let mut measured = 0;
    let result = Text::from_reader_snapshots(
        &mut reader,
        options.publish_bytes,
        options.publish_interval,
        |text| {
            cancel.check().map_err(io::Error::other)?;
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
                progress: Progress {
                    read: read.get(),
                    total,
                },
                metrics,
            });
            Ok(())
        },
    );
    let text = match result {
        _ if cancel.is_cancelled() => return Err(LoadError::Cancelled),
        Err(e) if e.kind() == io::ErrorKind::InvalidData => return Err(LoadError::DecodeRequired),
        other => other?,
    };
    Ok(Loaded {
        text,
        bom,
        progress: Progress {
            read: read.get(),
            total,
        },
        metrics,
    })
}
struct CheckedReader<'a, R> {
    inner: R,
    read: &'a Cell<u64>,
    cancel: &'a Cancellation,
    max_read: usize,
}
impl<R: Read> Read for CheckedReader<'_, R> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        self.cancel.check().map_err(io::Error::other)?;
        let cap = out.len().min(self.max_read);
        let n = self.inner.read(&mut out[..cap])?;
        self.read.set(self.read.get() + n as u64);
        Ok(n)
    }
}
/// Background task with a single latest-prefix mailbox and a separate terminal result.
/// A slow consumer never causes the worker to block or queue old snapshot roots.
#[derive(Debug)]
pub struct LoadTask {
    latest: Arc<Mutex<Option<LoadSnapshot>>>,
    read: Arc<AtomicU64>,
    cancel: Cancellation,
    /// Exactly one terminal result; disconnection indicates an unexpected worker panic.
    pub result: Receiver<Result<Loaded, LoadError>>,
    /// Initial source length. Loading still reads to EOF if this changes.
    pub total: u64,
}
impl LoadTask {
    /// Takes the newest prefix without waiting. A busy or empty mailbox returns
    /// None; retry on the next dispatcher tick.
    pub fn try_snapshot(&self) -> Option<LoadSnapshot> {
        self.latest.try_lock().ok()?.take()
    }
    /// Most recently published source byte progress, without acquiring the mailbox.
    pub fn progress(&self) -> Progress {
        Progress {
            read: self.read.load(Ordering::Acquire),
            total: Some(self.total),
        }
    }
    /// Requests cancellation; the terminal result will be Cancelled.
    pub fn cancel(&self) {
        self.cancel.cancel();
    }
}
impl Drop for LoadTask {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}
/// Opens a file and starts a cancellable UTF-8 worker; no UI or vel-fileio dependency.
pub fn load_utf8(path: impl AsRef<Path>, options: LoadOptions) -> Result<LoadTask, LoadError> {
    let file = File::open(path)?;
    let total = file.metadata()?.len();
    let latest = Arc::new(Mutex::new(None));
    let read = Arc::new(AtomicU64::new(0));
    let cancel = Cancellation::default();
    let task_latest = latest.clone();
    let task_read = read.clone();
    let task_cancel = cancel.clone();
    let (sender, result) = mpsc::sync_channel(1);
    thread::Builder::new()
        .name("vel-largefile-load".into())
        .spawn(move || {
            let loaded = stream_utf8(file, Some(total), &options, &task_cancel, |snapshot| {
                task_read.store(snapshot.progress.read, Ordering::Release);
                let previous = task_latest.lock().unwrap().replace(snapshot);
                drop(previous); // Release potentially large trees outside the mailbox lock.
            });
            if loaded.is_err() {
                let previous = task_latest.lock().unwrap().take();
                drop(previous);
            }
            let _ = sender.send(loaded);
        })?;
    Ok(LoadTask {
        latest,
        read,
        cancel,
        result,
        total,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn polling_a_busy_mailbox_never_waits_for_its_owner() {
        let (_sender, result) = mpsc::channel();
        let task = LoadTask {
            latest: Arc::new(Mutex::new(None)),
            read: Arc::new(AtomicU64::new(0)),
            cancel: Cancellation::default(),
            result,
            total: 0,
        };
        let mailbox = task.latest.clone();
        let guard = mailbox.lock().unwrap();
        let (sender, received) = mpsc::channel();
        let poller = thread::spawn(move || sender.send(task.try_snapshot().is_none()).unwrap());
        let response = received.recv_timeout(Duration::from_secs(1));
        drop(guard);
        poller.join().unwrap();
        assert!(response.expect("polling blocked on the mailbox"));
    }
}
