use std::{
    fs,
    process::{Command, Stdio},
};
use vel_crash::{Config, CrashReporter, Level, Report};

fn reporter(root: &std::path::Path) -> CrashReporter {
    let mut config = Config::for_home(root, "12.3.4");
    config.log_capacity = 2;
    CrashReporter::new(config).unwrap()
}

#[test]
fn reports_are_private_bounded_local_and_acknowledged_without_deletion() {
    let root = tempfile::tempdir().unwrap();
    let reporter = reporter(root.path());
    reporter.record(Level::Info, "old event discarded");
    reporter.record(Level::Warn, "resource load failed");
    reporter.record(Level::Error, "native initialization failed");
    let report = reporter.write_report().unwrap();
    let content = fs::read_to_string(&report.path).unwrap();
    assert!(content.contains("version: 12.3.4"));
    assert!(content.contains(&format!("os: {}", std::env::consts::OS)));
    assert!(content.contains("backtrace:\n"));
    assert!(content.contains("resource load failed"));
    assert!(content.contains("native initialization failed"));
    assert!(!content.contains("old event discarded"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&report.path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(reporter.directory())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
    }
    assert_eq!(
        reporter.pending_reports().unwrap(),
        std::slice::from_ref(&report)
    );
    let seen = reporter.acknowledge(&report).unwrap();
    assert!(seen.exists());
    assert!(reporter.pending_reports().unwrap().is_empty());
    let outside = root.path().join("outside.crash");
    fs::write(&outside, "leave alone").unwrap();
    assert!(
        reporter
            .acknowledge(&Report {
                path: outside.clone()
            })
            .is_err()
    );
    assert!(outside.exists());
}

#[test]
fn zero_capacity_log_and_unique_reports() {
    let root = tempfile::tempdir().unwrap();
    let mut config = Config::for_home(root.path(), "0.0.0");
    config.log_capacity = 0;
    let reporter = CrashReporter::new(config).unwrap();
    reporter.record(Level::Info, "discard this event");
    let first = reporter.write_report().unwrap();
    let second = reporter.write_report().unwrap();
    assert_ne!(first.path, second.path);
    assert!(
        !fs::read_to_string(first.path)
            .unwrap()
            .contains("discard this event")
    );
}

#[test]
fn crashing_child() {
    let Some(directory) = std::env::var_os("VELOCITY_CRASH_TEST_HOME") else {
        return;
    };
    let reporter = reporter(std::path::Path::new(&directory));
    reporter.install_panic_hook();
    reporter.record(Level::Info, "first retained event");
    #[cfg(unix)]
    if std::env::var("VELOCITY_CRASH_TEST_MODE").as_deref() == Ok("signal") {
        reporter.install_signal_handlers().unwrap();
        assert!(reporter.install_signal_handlers().is_err());
        reporter.record(Level::Warn, "last event after signal installation");
        // SAFETY: this isolated child process intentionally exercises the fatal handler.
        unsafe { libc::raise(libc::SIGSEGV) };
        unreachable!();
    }
    #[cfg(unix)]
    if std::env::var("VELOCITY_CRASH_TEST_MODE").as_deref() == Ok("overflow") {
        reporter.install_signal_handlers().unwrap();
        #[allow(unconditional_recursion)]
        #[inline(never)]
        fn overflow(depth: usize) -> usize {
            let bytes = std::hint::black_box([depth as u8; 8192]);
            overflow(depth + 1) + std::hint::black_box(usize::from(bytes[0]))
        }
        std::thread::Builder::new()
            .stack_size(128 * 1024)
            .spawn(|| {
                vel_crash::install_thread_stack().unwrap();
                std::hint::black_box(overflow(0))
            })
            .unwrap()
            .join()
            .unwrap();
        unreachable!();
    }
    if std::env::var("VELOCITY_CRASH_TEST_MODE").as_deref() == Ok("recoverable") {
        assert!(
            std::panic::catch_unwind(|| {
                vel_crash::recoverable(|| panic!("caught restore panic"))
            })
            .is_err()
        );
        return;
    }
    if std::env::var("VELOCITY_CRASH_TEST_MODE").as_deref() == Ok("normal") {
        #[cfg(unix)]
        reporter.install_signal_handlers().unwrap();
        return;
    }
    panic!("PRIVATE_DOCUMENT_TEXT_SHOULD_NOT_APPEAR");
}

fn run_child(mode: &str, home: &std::path::Path) -> std::process::ExitStatus {
    Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "crashing_child", "--nocapture"])
        .env("VELOCITY_CRASH_TEST_HOME", home)
        .env("VELOCITY_CRASH_TEST_MODE", mode)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap()
}

#[test]
fn panic_hook_omits_payload_and_next_launch_discovers_it() {
    let root = tempfile::tempdir().unwrap();
    assert!(!run_child("panic", root.path()).success());
    let next_launch = reporter(root.path());
    let reports = next_launch.pending_reports().unwrap();
    assert_eq!(reports.len(), 1);
    let text = fs::read_to_string(&reports[0].path).unwrap();
    assert!(text.contains("kind: panic"));
    assert!(text.contains("first retained event"));
    assert!(!text.contains("PRIVATE_DOCUMENT_TEXT_SHOULD_NOT_APPEAR"));
}

#[cfg(unix)]
#[test]
#[ignore = "native WIFSIGNALED check; the macOS sandbox can suspend OS crash exits"]
fn fatal_signal_preserves_sigsegv_exit_status() {
    use std::os::unix::process::ExitStatusExt;
    let root = tempfile::tempdir().unwrap();
    assert_eq!(
        run_child("signal", root.path()).signal(),
        Some(libc::SIGSEGV)
    );
    let next_launch = reporter(root.path());
    let reports = next_launch.pending_reports().unwrap();
    assert_eq!(reports.len(), 1);
    let text = fs::read_to_string(&reports[0].path).unwrap();
    assert!(text.contains("signal: SIGSEGV"));
    assert!(text.contains("version: 12.3.4"));
    assert!(text.contains("last event after signal installation"));
    assert!(text.contains("backtrace: unavailable in async signal context"));
}

#[cfg(unix)]
#[test]
fn normal_exit_removes_empty_signal_reservation() {
    let normal_home = tempfile::tempdir().unwrap();
    assert!(run_child("normal", normal_home.path()).success());
    assert!(
        reporter(normal_home.path())
            .pending_reports()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        fs::read_dir(reporter(normal_home.path()).directory())
            .unwrap()
            .count(),
        0
    );
}

#[cfg(unix)]
#[test]
#[ignore = "native WIFSIGNALED check; the macOS sandbox can suspend OS crash exits"]
fn stack_overflow_reports_on_an_alternate_stack() {
    use std::os::unix::process::ExitStatusExt;
    let home = tempfile::tempdir().unwrap();
    assert!(matches!(
        run_child("overflow", home.path()).signal(),
        Some(libc::SIGSEGV | libc::SIGBUS)
    ));
    assert_eq!(reporter(home.path()).pending_reports().unwrap().len(), 1);
}

#[test]
fn recoverable_panic_does_not_create_a_report() {
    let home = tempfile::tempdir().unwrap();
    assert!(run_child("recoverable", home.path()).success());
    assert!(reporter(home.path()).pending_reports().unwrap().is_empty());
}
