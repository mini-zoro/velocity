//! Local crash diagnostics with a bounded, static-message log and no document payloads.
//!
//! Panic payloads, runtime thread names, environment variables, document text and command
//! arguments are deliberately omitted. Signal reports use only async-signal-safe writes;
//! Rust backtraces are captured for panics, never from a fatal signal handler.
mod signals;

use std::{
    backtrace::Backtrace,
    collections::VecDeque,
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};
use tempfile::NamedTempFile;

thread_local! {
    static RECOVERING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Runs a recoverable job whose caller will report any panic as an ordinary error.
/// The crash hook ignores only this thread and only while the job is running.
pub fn catch_recoverable<F, T>(job: F) -> std::thread::Result<T>
where
    F: FnOnce() -> T + std::panic::UnwindSafe,
{
    let previous = RECOVERING.replace(true);
    let result = std::panic::catch_unwind(job);
    RECOVERING.set(previous);
    result
}

/// Configuration supplied early in app startup, before native initialization.
#[derive(Clone, Debug)]
pub struct Config {
    /// Normally `~/Library/Logs/Velocity`, never a document or session directory.
    pub directory: PathBuf,
    /// The app's build version, normally `env!("CARGO_PKG_VERSION")`.
    pub version: String,
    /// Maximum retained diagnostic events (clamped to 1024).
    pub log_capacity: usize,
}
impl Config {
    /// Resolves the conventional macOS location using the supplied home directory.
    /// Taking an explicit path makes startup tests independent of the real user's logs.
    pub fn for_home(home: &Path, version: impl Into<String>) -> Self {
        Self {
            directory: home.join("Library/Logs/Velocity"),
            version: version.into(),
            log_capacity: 100,
        }
    }
}
/// Diagnostic severity; messages contain only static code-authored strings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    /// Normal lifecycle activity.
    Info,
    /// Recoverable failure.
    Warn,
    /// Failure preceding a crash.
    Error,
}
#[derive(Debug)]
struct State {
    config: Config,
    os: String,
    logs: Mutex<VecDeque<String>>,
    signals_installed: AtomicBool,
    reporting: AtomicBool,
}

/// Shared local reporter. Cloning does not install hooks or create extra files.
#[derive(Clone, Debug)]
pub struct CrashReporter {
    state: Arc<State>,
}
/// A report that the next-launch UI can offer to reveal in Finder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Report {
    /// Absolute report path. Reports use mode 0600 on Unix.
    pub path: PathBuf,
}
impl CrashReporter {
    /// Creates a private log directory. Does not install global hooks or access the network.
    pub fn new(mut config: Config) -> io::Result<Self> {
        config.log_capacity = config.log_capacity.min(1024);
        if config.version.contains(['\n', '\r', '\0']) || config.version.len() > 128 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid application version",
            ));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(&config.directory)?;
        }
        #[cfg(not(unix))]
        fs::create_dir_all(&config.directory)?;
        config.directory = fs::canonicalize(&config.directory)?;
        Ok(Self {
            state: Arc::new(State {
                config,
                os: os_description(),
                logs: Mutex::new(VecDeque::new()),
                signals_installed: AtomicBool::new(false),
                reporting: AtomicBool::new(false),
            }),
        })
    }
    /// Directory to reveal after the next-launch prompt is accepted.
    pub fn directory(&self) -> &Path {
        &self.state.config.directory
    }
    /// Retains the last N code-authored event lines. Do not use leaked/constructed strings.
    /// There is intentionally no API accepting a runtime message, document, or formatting args.
    pub fn record(&self, level: Level, event: &'static str) {
        let mut logs = self.state.logs.lock().unwrap_or_else(|e| e.into_inner());
        let capacity = self.state.config.log_capacity;
        if capacity == 0 {
            return;
        }
        while logs.len() >= capacity {
            logs.pop_front();
        }
        let mut event: String = event
            .chars()
            .filter(|c| !c.is_control())
            .take(256)
            .collect();
        while event.len() > 256 {
            event.pop();
        }
        logs.push_back(format!("{:?} {event}", level));
        if self.state.signals_installed.load(Ordering::Acquire) {
            signals::publish(&self.context(&logs));
        }
    }
    fn context(&self, logs: &VecDeque<String>) -> String {
        let mut text = format!(
            "Velocity crash report\nversion: {}\nos: {}\nprocess: {}\nprivacy: no document contents; panic payload omitted\nlast log lines:\n",
            self.state.config.version,
            self.state.os,
            std::process::id()
        );
        for line in logs {
            text.push_str(line);
            text.push('\n');
        }
        text
    }
    /// Writes a synthetic diagnostic report without document data, useful for app diagnostics.
    pub fn write_report(&self) -> io::Result<Report> {
        self.write_report_at(None)
    }
    fn write_report_at(&self, location: Option<(&str, u32, u32)>) -> io::Result<Report> {
        let logs = self
            .state
            .logs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let mut text = self.context(&logs);
        text.push_str("kind: panic\n");
        if let Some((file, line, column)) = location {
            text.push_str(&format!("source: {file}:{line}:{column}\n"));
        }
        text.push_str(&format!("backtrace:\n{}\n", Backtrace::force_capture()));
        let mut temp = NamedTempFile::new_in(self.directory())?;
        temp.write_all(text.as_bytes())?;
        temp.as_file().sync_all()?;
        let path = self.report_path("panic", "crash");
        temp.persist_noclobber(&path).map_err(|e| e.error)?;
        Ok(Report { path })
    }
    /// Installs a process-global panic hook and chains the prior hook for stderr diagnostics.
    /// Install once at startup. The hook captures no panic payload or runtime thread name.
    pub fn install_panic_hook(&self) {
        let reporter = self.clone();
        let prior = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            if RECOVERING.get() {
                return;
            }
            if !reporter.state.reporting.swap(true, Ordering::AcqRel) {
                let location = info.location().map(|l| (l.file(), l.line(), l.column()));
                let _ = reporter.write_report_at(location);
                reporter.state.reporting.store(false, Ordering::Release);
            }
            prior(info);
        }));
    }
    /// Installs minimal fatal-signal handlers once per process, retaining an open report fd.
    /// SIGABRT/SIGBUS/SIGFPE/SIGILL/SIGSEGV terminate with status `128 + signal` after writing.
    /// Signal reports carry cached version/OS/logs and an explicit "backtrace unavailable" line.
    /// No locks, allocation, unwinding, formatting or network operations run in the handler.
    /// Empty reserved `.signal` files are ignored on normal exit and on subsequent launches.
    #[cfg(unix)]
    pub fn install_signal_handlers(&self) -> io::Result<()> {
        let logs = self.state.logs.lock().unwrap_or_else(|e| e.into_inner());
        signals::install(&self.report_path("fatal", "signal"), &self.context(&logs))?;
        self.state.signals_installed.store(true, Ordering::Release);
        Ok(())
    }
    /// Lists unacknowledged reports for the next-launch prompt. Empty signal reservations
    /// are ignored, including reservations owned by other concurrently running instances.
    pub fn pending_reports(&self) -> io::Result<Vec<Report>> {
        let mut reports = Vec::new();
        for entry in fs::read_dir(self.directory())? {
            let entry = entry?;
            if !entry.file_type()?.is_file() {
                continue;
            }
            let path = entry.path();
            let extension = path.extension().and_then(|s| s.to_str()).unwrap_or("");
            if extension != "crash" && extension != "signal" {
                continue;
            }
            if entry.metadata()?.len() == 0 {
                continue;
            }
            reports.push(Report { path });
        }
        reports.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(reports)
    }
    /// Marks a displayed/dismissed report as seen without deleting it. The UI decides whether
    /// to acknowledge after "Open Folder" or "Dismiss"; unrelated paths are rejected.
    pub fn acknowledge(&self, report: &Report) -> io::Result<PathBuf> {
        if report.path.parent() != Some(self.directory())
            || !matches!(
                report.path.extension().and_then(|s| s.to_str()),
                Some("crash" | "signal")
            )
            || !fs::symlink_metadata(&report.path)?.file_type().is_file()
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "not a local crash report",
            ));
        }
        let seen = report.path.with_extension("seen");
        if seen.exists() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "report already acknowledged",
            ));
        }
        fs::rename(&report.path, &seen)?;
        Ok(seen)
    }
    fn report_path(&self, kind: &str, extension: &str) -> PathBuf {
        static SEQUENCE: AtomicU64 = AtomicU64::new(0);
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        self.directory().join(format!(
            "{timestamp}-{}-{}-{kind}.{extension}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ))
    }
}
fn os_description() -> String {
    let mut description = format!("{} {}", std::env::consts::OS, std::env::consts::ARCH);
    #[cfg(target_os = "macos")]
    if let Ok(output) = std::process::Command::new("/usr/bin/sw_vers")
        .arg("-productVersion")
        .output()
        && output.status.success()
    {
        let version = String::from_utf8_lossy(&output.stdout);
        description.push(' ');
        description.extend(
            version
                .trim()
                .chars()
                .filter(|c| c.is_ascii_digit() || *c == '.')
                .take(32),
        );
    }
    description
}
