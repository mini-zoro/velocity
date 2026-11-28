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
// The reader serializes parser state separately from the UI mailbox. Every
// publication is a pipe-sized batch; no regex, text scan, trim/copy or destruction
// of a discarded backlog runs under the mailbox lock.
#[derive(Debug)]
struct Chunk {
    stream: Stream,
    text: String,
    start: usize,
    lines: usize,
    first_lf: Option<usize>,
    ends_lf: bool,
}
impl Chunk {
    fn new(stream: Stream, text: String) -> Self {
        let lines = text.bytes().filter(|b| *b == b'\n').count();
        let first_lf = text.find('\n');
        let ends_lf = text.ends_with('\n');
        Self {
            stream,
            text,
            start: 0,
            lines,
            first_lf,
            ends_lf,
        }
    }
    fn len(&self) -> usize {
        self.text.len() - self.start
    }
    fn into_output(mut self) -> (Stream, String) {
        if self.start > 0 {
            self.text.drain(..self.start);
        }
        (self.stream, self.text)
    }
}
#[derive(Debug)]
struct Parsing {
    parser: ResultParser,
    pending: String,
    seen: usize,
    errors: usize,
}
impl Parsing {
    fn parse(&mut self, text: &str, finish: bool) -> Vec<(Arc<Diagnostic>, usize)> {
        self.seen += text.len();
        let mut diagnostics = Vec::new();
        for piece in text.split_inclusive('\n') {
            self.pending.push_str(piece);
            if self.pending.len() > 1024 * 1024 {
                let cut = self
                    .pending
                    .ceil_char_boundary(self.pending.len() - 1024 * 1024);
                self.pending.drain(..cut);
            }
            if piece.ends_with('\n') {
                self.line(&mut diagnostics);
            }
        }
        if finish && !self.pending.is_empty() {
            self.line(&mut diagnostics);
        }
        diagnostics
    }
    fn line(&mut self, diagnostics: &mut Vec<(Arc<Diagnostic>, usize)>) {
        if let Some(diagnostic) = self.parser.parse(&self.pending) {
            self.errors += 1;
            diagnostics.push((Arc::new(diagnostic), self.seen));
        }
        self.pending.clear();
    }
}
fn diagnostic_bytes(d: &Diagnostic) -> usize {
    d.message.len() + d.path.as_os_str().len() + std::mem::size_of::<Diagnostic>()
}
#[derive(Debug)]
struct State {
    chunks: VecDeque<Chunk>,
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
    errors: usize,
    diagnostics: VecDeque<(Arc<Diagnostic>, usize)>,
    diagnostic_bytes: usize,
}
// Moving ownership out of the mailbox keeps text trimming, diagnostics cloning
// and freeing a cancelled/evicted backlog off its short critical section.
enum Delivery {
    Event(Event),
    Output(Chunk),
    Batch(VecDeque<Chunk>),
    Results(usize, VecDeque<(Arc<Diagnostic>, usize)>),
    Cancel(usize, VecDeque<Chunk>, VecDeque<(Arc<Diagnostic>, usize)>),
}
impl Delivery {
    fn event(self) -> Event {
        match self {
            Self::Event(event) => event,
            Self::Output(chunk) => {
                let (stream, text) = chunk.into_output();
                Event::Output { stream, text }
            }
            Self::Batch(chunks) => {
                Event::OutputBatch(chunks.into_iter().map(Chunk::into_output).collect())
            }
            Self::Results(errors, diagnostics) => Event::Results(Results {
                reset: true,
                errors,
                diagnostics: diagnostics
                    .into_iter()
                    .map(|(d, _)| Arc::unwrap_or_clone(d))
                    .collect(),
            }),
            Self::Cancel(errors, chunks, diagnostics) => {
                drop((chunks, diagnostics));
                Event::Results(Results {
                    reset: false,
                    errors,
                    diagnostics: vec![],
                })
            }
        }
    }
}
impl State {
    fn discard(&mut self, chunk: &Chunk) {
        self.dropped += chunk.len();
        self.row += chunk.lines;
        self.reset = true;
    }
    fn publish(&mut self, mut chunk: Chunk, retired: &mut Vec<Chunk>) {
        if self.partial {
            if let Some(lf) = chunk.first_lf {
                self.dropped += lf + 1;
                self.row += 1;
                self.reset = true;
                chunk.start = lf + 1;
                chunk.lines -= 1;
                self.partial = false;
            } else {
                self.discard(&chunk);
                retired.push(chunk);
                return;
            }
        }
        if chunk.len() > 0 {
            self.bytes += chunk.len();
            self.chunks.push_back(chunk);
        }
        if self.bytes > LIMIT {
            while self.bytes > KEEP {
                let chunk = self.chunks.pop_front().expect("queued bytes");
                self.bytes -= chunk.len();
                self.partial = !chunk.ends_lf;
                self.discard(&chunk);
                retired.push(chunk);
            }
            while self.partial {
                let Some(mut chunk) = self.chunks.pop_front() else {
                    break;
                };
                self.bytes -= chunk.len();
                if let Some(lf) = chunk.first_lf.filter(|lf| *lf >= chunk.start) {
                    self.dropped += lf + 1 - chunk.start;
                    self.row += 1;
                    self.reset = true;
                    chunk.start = lf + 1;
                    chunk.lines -= 1;
                    self.bytes += chunk.len();
                    self.chunks.push_front(chunk);
                    self.partial = false;
                } else {
                    self.discard(&chunk);
                    retired.push(chunk);
                }
            }
        }
        self.peak = self.peak.max(self.bytes);
    }
    fn next(&mut self, cancel: bool, started: Instant) -> Result<Delivery, TryRecvError> {
        // A cancelled build discards unread output, but must still report a
        // retention drop that had already happened before cancellation.
        if self.reset {
            self.reset = false;
            return Ok(Delivery::Event(Event::Truncated {
                bytes: self.dropped,
                first_line: self.row,
                cancelling: cancel,
            }));
        }
        if cancel && !self.cancelled_delivered {
            self.bytes = 0;
            self.messages.clear();
            self.finished = None;
            if !self.results_delivered {
                self.results_delivered = true;
                self.diagnostic_bytes = 0;
                return Ok(Delivery::Cancel(
                    self.errors,
                    std::mem::take(&mut self.chunks),
                    std::mem::take(&mut self.diagnostics),
                ));
            }
            self.cancelled_delivered = true;
            return Ok(Delivery::Event(Event::Finished {
                code: None,
                signal: None,
                cancelled: true,
                elapsed: started.elapsed(),
            }));
        }
        if self.cancelled_delivered {
            return Err(TryRecvError::Disconnected);
        }
        if let Some(event) = self.messages.pop_front() {
            return Ok(Delivery::Event(event));
        }
        if self.finished.is_some() && self.bytes > 65536 {
            self.row += self.chunks.iter().map(|c| c.lines).sum::<usize>();
            self.bytes = 0;
            return Ok(Delivery::Batch(std::mem::take(&mut self.chunks)));
        }
        if let Some(chunk) = self.chunks.pop_front() {
            self.bytes -= chunk.len();
            self.row += chunk.lines;
            return Ok(Delivery::Output(chunk));
        }
        if self.finished.is_some() && !self.results_delivered {
            self.results_delivered = true;
            self.diagnostic_bytes = 0;
            return Ok(Delivery::Results(
                self.errors,
                std::mem::take(&mut self.diagnostics),
            ));
        }
        if let Some(event) = self.finished.take() {
            return Ok(Delivery::Event(event));
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
    parsing: Mutex<Parsing>,
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
        if self.0.cancel.load(Ordering::Acquire) {
            if matches!(event, Event::Finished { .. }) {
                self.0.done.store(true, Ordering::Release);
            }
            return false;
        }
        // Only producers acquire this mutex. Holding it until publication preserves
        // stdout/stderr order and parser row coordinates without blocking the UI.
        let mut parsing = self.0.parsing.lock().unwrap();
        let (chunk, diagnostics, event) = match event {
            Event::Output { stream, text } => {
                let diagnostics = if stream == Stream::Input {
                    // An input echo can follow a prompt without a newline. Finish the
                    // output portion of that row, but never parse the user's line itself.
                    parsing.seen += text.len();
                    if parsing.pending.is_empty() {
                        parsing.parser.skip_row();
                        Vec::new()
                    } else {
                        parsing.parse("", true)
                    }
                } else {
                    parsing.parse(&text, false)
                };
                (Some(Chunk::new(stream, text)), diagnostics, None)
            }
            event @ Event::Finished { .. } => (None, parsing.parse("", true), Some(event)),
            event => (None, vec![], Some(event)),
        };
        let mut retired = Vec::new();
        let mut retired_diagnostics = Vec::new();
        let mut s = self.0.state.lock().unwrap();
        if s.closed || self.0.cancel.load(Ordering::Acquire) {
            return false;
        }
        s.errors = parsing.errors;
        for (d, at) in diagnostics {
            s.diagnostic_bytes += diagnostic_bytes(&d);
            s.diagnostics.push_back((d, at));
        }
        while s
            .diagnostics
            .front()
            .is_some_and(|(_, at)| parsing.seen.saturating_sub(*at) > LIMIT)
            || s.diagnostics.len() > 32_768
            || s.diagnostic_bytes > LIMIT * 2
        {
            let (d, _) = s.diagnostics.pop_front().expect("retained diagnostics");
            s.diagnostic_bytes -= diagnostic_bytes(&d);
            retired_diagnostics.push(d);
        }
        if let Some(chunk) = chunk {
            s.publish(chunk, &mut retired);
        }
        if let Some(event) = event {
            if matches!(event, Event::Finished { .. }) {
                s.finished = Some(event);
                self.0.done.store(true, Ordering::Release);
            } else {
                s.messages.push_back(event);
            }
        }
        drop(s);
        self.0.ready.notify_all();
        drop((retired, retired_diagnostics));
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
    fn delivered(&self, delivery: Delivery) -> Event {
        let event = delivery.event();
        if let Event::Truncated {
            first_line,
            cancelling: false,
            ..
        } = &event
        {
            self.next_result_row.store(*first_line, Ordering::Relaxed);
        }
        event
    }
    /// Receives ready output without waiting for parsing or treating publication as no data.
    pub fn try_recv(&self) -> Result<Event, TryRecvError> {
        let delivery = self.shared.state.lock().unwrap().next(
            self.shared.cancel.load(Ordering::Acquire),
            self.shared.started,
        )?;
        Ok(self.delivered(delivery))
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
                Ok(delivery) => {
                    drop(s);
                    return Ok(self.delivered(delivery));
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
    pub fn results(&self, through_row: usize) -> Option<Results> {
        let s = self.shared.state.lock().unwrap();
        if s.results_delivered {
            return None;
        }
        let row = self.next_result_row.load(Ordering::Relaxed);
        let first = s.diagnostics.partition_point(|(d, _)| d.output_line < row);
        let retained = s
            .diagnostics
            .iter()
            .skip(first)
            .take_while(|(d, _)| d.output_line < through_row)
            .take(256)
            .map(|(d, _)| d.clone())
            .collect::<Vec<_>>();
        let errors = s.errors;
        drop(s);
        let diagnostics = retained
            .into_iter()
            .map(|d| (*d).clone())
            .collect::<Vec<_>>();
        if let Some(last) = diagnostics.last() {
            self.next_result_row
                .store(last.output_line + 1, Ordering::Relaxed);
        }
        if diagnostics.is_empty() && self.reported_errors.load(Ordering::Relaxed) == errors {
            return None;
        }
        self.reported_errors.store(errors, Ordering::Relaxed);
        Some(Results {
            reset: false,
            errors,
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
            errors: 0,
            diagnostics: VecDeque::new(),
            diagnostic_bytes: 0,
        }),
        parsing: Mutex::new(Parsing {
            parser,
            pending: String::new(),
            seen: 0,
            errors: 0,
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
    /// Hang guard: stops a wait that would otherwise never end. It is not a latency
    /// bound; the test checks the condition it waits for.
    const HANG_GUARD: std::time::Duration = std::time::Duration::from_secs(120);

    #[test]
    fn cancelling_a_stalled_receiver_delivers_pending_truncation() {
        let cancel = Arc::new(AtomicBool::new(false));
        let (output, events) = channel(
            ResultParser::new(None, None, "/tmp".into()).unwrap(),
            Arc::new(AtomicBool::new(false)),
            cancel.clone(),
        );
        let chunk = "x".repeat(65535) + "\n";
        for _ in 0..=LIMIT / chunk.len() {
            assert!(output.send(Event::Output {
                stream: Stream::Stdout,
                text: chunk.clone(),
            }));
        }
        let dropped = events.stats().dropped_bytes;
        assert!(dropped > 0);
        cancel.store(true, Ordering::Release);
        assert!(
            matches!(events.try_recv(), Ok(Event::Truncated { bytes, .. }) if bytes == dropped)
        );
        assert!(matches!(events.try_recv(), Ok(Event::Results(_))));
        assert!(matches!(
            events.try_recv(),
            Ok(Event::Finished {
                cancelled: true,
                ..
            })
        ));
    }

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
                match events.recv_timeout(HANG_GUARD).unwrap() {
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
