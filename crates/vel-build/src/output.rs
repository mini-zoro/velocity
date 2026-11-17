//! Bounded build transport. Retain a tail in RAM; no append-only disk spool.
use crate::{Diagnostic, Event, ResultParser, Stream};
use std::{
    collections::VecDeque,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc::{RecvTimeoutError, TryRecvError},
    },
    time::{Duration, Instant},
};
const LIMIT: usize = 8 * 1024 * 1024;
const KEEP: usize = 6 * 1024 * 1024;
/// Worker-parsed results, independent of skipped output replay.
#[derive(Debug)]
pub struct Results {
    /// Replace the visible result list for a completed retained tail.
    pub reset: bool,
    /// Total matches, including diagnostics in discarded output.
    pub errors: usize,
    /// Most recent navigable diagnostics in absolute output-row coordinates.
    pub diagnostics: Vec<Diagnostic>,
}
/// Transport bounds exposed for headless verification.
#[derive(Debug)]
pub struct OutputStats {
    /// Queued UTF-8 bytes; at most eight MiB.
    pub buffered_bytes: usize,
    /// Largest queued text size observed.
    pub peak_bytes: usize,
    /// Bytes discarded before delivery.
    pub dropped_bytes: usize,
    /// No spill file is created by this transport.
    pub spill_file_bytes: u64,
    /// Retained result text/path/record bytes; at most sixteen MiB.
    pub diagnostic_bytes: usize,
}
#[derive(Debug)]
struct State {
    chunks: VecDeque<(Stream, String)>,
    messages: VecDeque<Event>,
    bytes: usize,
    peak: usize,
    dropped: usize,
    reset: bool,
    partial: bool,
    row: usize,
    finished: Option<Event>,
    cancelled_delivered: bool,
    results_delivered: bool,
    senders: usize,
    closed: bool,
    parser: ResultParser,
    pending: String,
    errors: usize,
    diagnostics: VecDeque<(Diagnostic, usize)>,
    diagnostic_bytes: usize,
    seen: usize,
}
impl State {
    fn parse(&mut self, text: &str) {
        self.seen += text.len();
        for piece in text.split_inclusive('\n') {
            self.pending.push_str(piece);
            if self.pending.len() > 1024 * 1024 {
                let cut = self
                    .pending
                    .ceil_char_boundary(self.pending.len() - 1024 * 1024);
                self.pending.drain(..cut);
            }
            if piece.ends_with('\n') {
                self.parse_pending();
            }
        }
        while self
            .diagnostics
            .front()
            .is_some_and(|(_, at)| self.seen.saturating_sub(*at) > LIMIT)
        {
            self.pop_diagnostic();
        }
    }
    fn parse_pending(&mut self) {
        if let Some(diagnostic) = self.parser.parse(&self.pending) {
            self.errors += 1;
            self.diagnostic_bytes += diagnostic.message.len()
                + diagnostic.path.as_os_str().len()
                + std::mem::size_of::<Diagnostic>();
            self.diagnostics.push_back((diagnostic, self.seen));
            while self.diagnostics.len() > 32_768 || self.diagnostic_bytes > LIMIT * 2 {
                self.pop_diagnostic();
            }
        }
        self.pending.clear();
    }
    fn pop_diagnostic(&mut self) {
        if let Some((diagnostic, _)) = self.diagnostics.pop_front() {
            self.diagnostic_bytes -= diagnostic.message.len()
                + diagnostic.path.as_os_str().len()
                + std::mem::size_of::<Diagnostic>();
        }
    }
    fn discard(&mut self, text: &str) {
        self.dropped += text.len();
        self.row += text.matches('\n').count();
        self.reset = true;
    }
    fn trim(&mut self) {
        if self.bytes <= LIMIT {
            return;
        }
        while self.bytes > KEEP {
            let (stream, mut text) = self.chunks.pop_front().expect("queued bytes");
            self.bytes -= text.len();
            if self.bytes < KEEP {
                let cut = text.len().saturating_sub(KEEP.saturating_sub(self.bytes));
                if let Some(lf) = text.as_bytes()[cut..].iter().position(|b| *b == b'\n') {
                    let cut = cut + lf + 1;
                    self.discard(&text[..cut]);
                    text.drain(..cut);
                    self.bytes += text.len();
                    self.chunks.push_front((stream, text));
                    self.partial = false;
                    break;
                }
            }
            self.partial = !text.ends_with('\n');
            self.discard(&text);
        }
        // The next retained row must start at a line boundary, including when a
        // single pathological line spans multiple reads or exceeds the whole limit.
        while self.partial {
            let Some((stream, mut text)) = self.chunks.pop_front() else {
                break;
            };
            self.bytes -= text.len();
            if let Some(lf) = text.find('\n') {
                self.discard(&text[..lf + 1]);
                text.drain(..lf + 1);
                self.bytes += text.len();
                self.chunks.push_front((stream, text));
                self.partial = false;
            } else {
                self.discard(&text);
            }
        }
    }
    fn take_results(&mut self) -> Results {
        self.diagnostic_bytes = 0;
        Results {
            reset: true,
            errors: self.errors,
            diagnostics: self.diagnostics.drain(..).map(|(d, _)| d).collect(),
        }
    }
    fn next(&mut self, cancel: bool, started: Instant) -> Result<Event, TryRecvError> {
        if cancel && !self.cancelled_delivered {
            self.chunks.clear();
            self.bytes = 0;
            self.messages.clear();
            self.finished = None;
            if !self.results_delivered {
                self.results_delivered = true;
                self.diagnostics.clear();
                self.diagnostic_bytes = 0;
                return Ok(Event::Results(Results {
                    reset: false,
                    errors: self.errors,
                    diagnostics: vec![],
                }));
            }
            self.cancelled_delivered = true;
            return Ok(Event::Finished {
                code: None,
                signal: None,
                cancelled: true,
                elapsed: started.elapsed(),
            });
        }
        if self.cancelled_delivered {
            return Err(TryRecvError::Disconnected);
        }
        if self.reset {
            self.reset = false;
            return Ok(Event::Truncated {
                bytes: self.dropped,
                first_line: self.row,
            });
        }
        if let Some(event) = self.messages.pop_front() {
            return Ok(event);
        }
        if self.finished.is_some() && self.bytes > 65536 {
            self.row += self
                .chunks
                .iter()
                .map(|(_, text)| text.matches('\n').count())
                .sum::<usize>();
            self.bytes = 0;
            return Ok(Event::OutputBatch(self.chunks.drain(..).collect()));
        }
        if let Some((stream, text)) = self.chunks.pop_front() {
            self.bytes -= text.len();
            self.row += text.matches('\n').count();
            return Ok(Event::Output { stream, text });
        }
        if self.finished.is_some() && !self.results_delivered {
            self.results_delivered = true;
            return Ok(Event::Results(self.take_results()));
        }
        if let Some(event) = self.finished.take() {
            return Ok(event);
        }
        Err(if self.senders == 0 {
            TryRecvError::Disconnected
        } else {
            TryRecvError::Empty
        })
    }
}
#[derive(Debug)]
struct Shared {
    state: Mutex<State>,
    ready: Condvar,
    done: Arc<AtomicBool>,
    cancel: Arc<AtomicBool>,
    started: Instant,
}
#[derive(Debug)]
pub(crate) struct Sender(Arc<Shared>);
impl Clone for Sender {
    fn clone(&self) -> Self {
        self.0.state.lock().unwrap().senders += 1;
        Self(self.0.clone())
    }
}
impl Drop for Sender {
    fn drop(&mut self) {
        self.0.state.lock().unwrap().senders -= 1;
        self.0.ready.notify_all();
    }
}
impl Sender {
    pub(crate) fn send(&self, event: Event) -> bool {
        let mut s = self.0.state.lock().unwrap();
        if s.closed || self.0.cancel.load(Ordering::Acquire) {
            if matches!(event, Event::Finished { .. }) {
                self.0.done.store(true, Ordering::Release);
            }
            return false;
        }
        match event {
            Event::Output { stream, text } => {
                s.parse(&text);
                let displayed = if s.partial {
                    if let Some(lf) = text.find('\n') {
                        s.discard(&text[..lf + 1]);
                        s.partial = false;
                        &text[lf + 1..]
                    } else {
                        s.discard(&text);
                        ""
                    }
                } else {
                    &text
                };
                if !displayed.is_empty() {
                    s.bytes += displayed.len();
                    if let Some((previous, tail)) = s.chunks.back_mut()
                        && *previous == stream
                        && tail.len() + displayed.len() <= 65536
                    {
                        tail.push_str(displayed);
                    } else {
                        s.chunks.push_back((stream, displayed.to_owned()));
                    }
                    s.trim();
                    s.peak = s.peak.max(s.bytes);
                }
            }
            event @ Event::Finished { .. } => {
                if !s.pending.is_empty() {
                    s.parse_pending();
                }
                s.finished = Some(event);
                self.0.done.store(true, Ordering::Release);
            }
            event => s.messages.push_back(event),
        }
        self.0.ready.notify_all();
        true
    }
}
/// Bounded, cancellable receiver. UI polling never waits for parsing or pipe I/O.
#[derive(Debug)]
pub struct Events {
    shared: Arc<Shared>,
    next_result_row: AtomicUsize,
    reported_errors: AtomicUsize,
}
impl Events {
    /// Receives ready output without blocking the UI on a producer.
    pub fn try_recv(&self) -> Result<Event, TryRecvError> {
        let Ok(mut s) = self.shared.state.try_lock() else {
            return Err(TryRecvError::Empty);
        };
        let event = s.next(
            self.shared.cancel.load(Ordering::Acquire),
            self.shared.started,
        );
        if let Ok(Event::Truncated { first_line, .. }) = &event {
            self.next_result_row.store(*first_line, Ordering::Relaxed);
        }
        event
    }
    /// Waits for output on worker/test callers.
    pub fn recv_timeout(&self, duration: Duration) -> Result<Event, RecvTimeoutError> {
        let deadline = Instant::now() + duration;
        let mut s = self.shared.state.lock().unwrap();
        loop {
            match s.next(
                self.shared.cancel.load(Ordering::Acquire),
                self.shared.started,
            ) {
                Ok(event) => {
                    if let Event::Truncated { first_line, .. } = &event {
                        self.next_result_row.store(*first_line, Ordering::Relaxed);
                    }
                    return Ok(event);
                }
                Err(TryRecvError::Disconnected) => return Err(RecvTimeoutError::Disconnected),
                Err(TryRecvError::Empty) => {}
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(RecvTimeoutError::Timeout);
            }
            s = self
                .shared
                .ready
                .wait_timeout(s, remaining.min(Duration::from_millis(25)))
                .unwrap()
                .0;
        }
    }
    /// At most 256 newly parsed diagnostics through the displayed complete rows.
    /// Completion moves one final retained snapshot into the event stream instead.
    pub fn results(&self, through_row: usize) -> Option<Results> {
        let s = self.shared.state.try_lock().ok()?;
        if s.results_delivered {
            return None;
        }
        let row = self.next_result_row.load(Ordering::Relaxed);
        let first = s.diagnostics.partition_point(|(d, _)| d.output_line < row);
        let diagnostics = s
            .diagnostics
            .iter()
            .skip(first)
            .take_while(|(d, _)| d.output_line < through_row)
            .take(256)
            .map(|(d, _)| d.clone())
            .collect::<Vec<_>>();
        if let Some(last) = diagnostics.last() {
            self.next_result_row
                .store(last.output_line + 1, Ordering::Relaxed);
        }
        if diagnostics.is_empty() && self.reported_errors.load(Ordering::Relaxed) == s.errors {
            return None;
        }
        self.reported_errors.store(s.errors, Ordering::Relaxed);
        Some(Results {
            reset: false,
            errors: s.errors,
            diagnostics,
        })
    }
    /// Observes transport bounds without reading or replaying its output.
    pub fn stats(&self) -> OutputStats {
        let s = self.shared.state.lock().unwrap();
        OutputStats {
            buffered_bytes: s.bytes,
            peak_bytes: s.peak,
            dropped_bytes: s.dropped,
            spill_file_bytes: 0,
            diagnostic_bytes: s.diagnostic_bytes,
        }
    }
}
impl Drop for Events {
    fn drop(&mut self) {
        self.shared.state.lock().unwrap().closed = true;
        self.shared.ready.notify_all();
    }
}
pub(crate) fn channel(
    parser: ResultParser,
    done: Arc<AtomicBool>,
    cancel: Arc<AtomicBool>,
) -> (Sender, Events) {
    let shared = Arc::new(Shared {
        state: Mutex::new(State {
            chunks: VecDeque::new(),
            messages: VecDeque::new(),
            bytes: 0,
            peak: 0,
            dropped: 0,
            reset: false,
            partial: false,
            row: 0,
            finished: None,
            cancelled_delivered: false,
            results_delivered: false,
            senders: 1,
            closed: false,
            parser,
            pending: String::new(),
            errors: 0,
            diagnostics: VecDeque::new(),
            diagnostic_bytes: 0,
            seen: 0,
        }),
        ready: Condvar::new(),
        done,
        cancel,
        started: Instant::now(),
    });
    (
        Sender(shared.clone()),
        Events {
            shared,
            next_result_row: AtomicUsize::new(0),
            reported_errors: AtomicUsize::new(0),
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn round18_final_spill_is_bounded_lossless_and_ordered() {
        for (count, width) in [(80, 65536), (1000, 1)] {
            let (output, events) = channel(
                ResultParser::new(None, None, "/tmp".into()).unwrap(),
                Arc::new(AtomicBool::new(false)),
                Arc::new(AtomicBool::new(false)),
            );
            let expected = (0..count)
                .map(|n| {
                    (
                        if n % 2 == 0 {
                            Stream::Stdout
                        } else {
                            Stream::Stderr
                        },
                        format!("{n:04}:{}\n", "x".repeat(width)),
                    )
                })
                .collect::<Vec<_>>();
            for (stream, text) in &expected {
                assert!(output.send(Event::Output {
                    stream: *stream,
                    text: text.clone()
                }));
            }
            assert!(events.stats().buffered_bytes <= LIMIT);
            assert_eq!(events.stats().spill_file_bytes, 0);
            assert!(output.send(Event::Finished {
                code: Some(0),
                signal: None,
                cancelled: false,
                elapsed: Duration::ZERO
            }));
            drop(output);
            let mut actual = Vec::new();
            loop {
                match events.recv_timeout(Duration::from_secs(10)).unwrap() {
                    Event::Output { stream, text } => actual.push((stream, text)),
                    Event::OutputBatch(chunks) => actual.extend(chunks),
                    Event::Finished { .. } => break,
                    Event::Results(_) => {}
                    other => panic!("unexpected {other:?}"),
                }
            }
            assert_eq!(actual, expected);
        }
    }
}
