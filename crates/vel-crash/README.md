# vel-crash

Local crash reporting for startup integration. No network client or upload API exists.

```rust,no_run
use vel_crash::{Config, CrashReporter, Level};

let home = std::path::PathBuf::from(std::env::var_os("HOME").expect("home directory"));
let reporter = CrashReporter::new(Config::for_home(&home, env!("CARGO_PKG_VERSION")))?;
// Collect previous reports before installing this launch's handlers.
let pending = reporter.pending_reports()?;
reporter.install_panic_hook();
#[cfg(unix)]
reporter.install_signal_handlers()?;
reporter.record(Level::Info, "application startup");
// After the app's prompt is handled, reveal reporter.directory() if accepted and
// reporter.acknowledge(&report) for each report shown/dismissed by that prompt.
# Ok::<(), std::io::Error>(())
```

`Config::for_home` selects `~/Library/Logs/Velocity`, with 100 log lines by default. `new` creates
a private directory (0700 on Unix). Reports use unique filenames and 0600 permissions, are
written via a synced temporary file and atomic rename, and include app version, OS/version,
process id, compiler source location, a forced Rust backtrace and bounded diagnostic history.
Panic payloads, document contents, environment, command arguments and runtime thread names
are omitted. `record(Level, &'static str)` deliberately cannot accept formatted/runtime log
messages; use only static code-authored lifecycle events and never leak strings into it.
`write_report()` provides a synthetic report for diagnostics. Paths in backtraces can identify
source/build locations; these remain local.

Install the panic hook once, early on the main thread. It retains the reporter and chains the
previous hook's stderr behavior. The library never redirects stderr into a report, so the
default panic payload is not copied into a crash file. The app should likewise avoid capturing
the prior hook's stderr as document-safe diagnostic history.

`install_signal_handlers()` is explicit and process-global. On Unix it handles SIGABRT,
SIGBUS, SIGFPE, SIGILL and SIGSEGV with a retained, preopened report descriptor, a bounded
atomic context buffer, POSIX `write`, and `_exit(128 + signal)`. Formatting, allocation, locks,
backtrace capture and normal Rust I/O occur outside the handler. The reserved descriptor remains
open until process exit to prevent concurrent signals from writing to a recycled descriptor.
Reports state that a backtrace is unavailable in signal context. Context is best effort if a
signal interrupts a log update; memory corruption, stack exhaustion, SIGKILL and power failure
cannot be guaranteed to produce a report. The handler exits rather than producing a core dump.
Normal launches can leave an empty `.signal` reservation; these are ignored, including while
other profiles are running, so a normal exit never triggers a next-launch crash prompt.

`pending_reports()` returns unacknowledged nonempty `.crash`/`.signal` files in deterministic
order. It does not include symlinks. `acknowledge(&Report)` renames a local report to `.seen`
without deleting it; it rejects unrelated paths. Lane A owns the next-launch "Open Report
Folder / Dismiss" prompt and uses the native folder-opening action only after user acceptance.
Report retention/deletion stays under user control.

Tests run actual panics and fatal signals in isolated subprocesses, then simulate the next
launch. They check privacy, log bounds, modes, backtraces, latest signal context, duplicate
signal installation, normal exits, unique names and acknowledgement. No test crashes the
parent test runner or writes into the real user's log directory.
