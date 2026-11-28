use super::*;
use crate::save_report_with_expected;
use std::{cell::RefCell, sync::mpsc};

/// Hang guard: stops a wait that would otherwise never end. It is not a latency
/// bound; the test checks the condition it waits for.
const HANG_GUARD: std::time::Duration = std::time::Duration::from_secs(120);

thread_local! {
    static WRITER: RefCell<Option<(mpsc::SyncSender<()>, mpsc::Receiver<()>)>> = const { RefCell::new(None) };
    static WAITER: RefCell<Option<mpsc::SyncSender<()>>> = const { RefCell::new(None) };
    static MTIME: RefCell<Option<(PathBuf, SystemTime)>> = const { RefCell::new(None) };
}

pub(super) fn simultaneous_saves(target: &Path, simulated: bool) {
    let profiles = [tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap()];
    let format = Format::default();
    // Use separate profiles like two independent App instances. Try both a
    // library caller without a disk digest and an App-style atomic contender.
    for (expect_disk, atomic) in [(false, false), (true, true)] {
        let old = "original".repeat(65_536);
        let first = "accepted".repeat(65_536);
        let second = Text::from("rejected".repeat(65_536).as_str());
        fs::write(target, &old).unwrap();
        let before = target.metadata().unwrap();
        let mtime = before.modified().unwrap();
        let disk = crate::content_identity(target).unwrap();
        let (at_write, at_write_rx) = mpsc::sync_channel(1);
        let (continue_write, waiting) = mpsc::sync_channel(1);
        std::thread::scope(|scope| {
            let winning_profile = profiles[0].path();
            let first = &first;
            let writer = scope.spawn(move || {
                WRITER.set(Some((at_write, waiting)));
                MTIME.set(Some((target.to_path_buf(), mtime)));
                use_faults(
                    &[
                        (Phase::Backup, unsupported),
                        (Phase::BeforeWrite, || {
                            let (ready, proceed) = WRITER.take().unwrap();
                            ready.send(()).unwrap();
                            proceed.recv_timeout(HANG_GUARD).unwrap();
                            Ok(())
                        }),
                        (Phase::AfterSync, || {
                            let (path, time) = MTIME.take().unwrap();
                            File::options().write(true).open(path)?.set_modified(time)
                        }),
                    ],
                    || save(target, winning_profile, first),
                )
                .unwrap();
            });
            at_write_rx.recv_timeout(HANG_GUARD).unwrap();
            WAITER.set(Some(continue_write));
            COARSE_VERSION_PROBE.set(simulated);
            let result = use_faults(
                &[(Phase::Waiting, || {
                    WAITER.take().unwrap().send(()).unwrap();
                    Ok(())
                })],
                || {
                    save_report_with_expected(
                        target,
                        &second,
                        &format,
                        atomic,
                        Some(profiles[1].path()),
                        disk.filter(|_| expect_disk),
                    )
                },
            );
            writer.join().unwrap();
            assert!(
                result
                    .unwrap_err()
                    .contains("file changed while staging save"),
                "second save must fail with the usual conflict"
            );
            if simulated {
                assert!(crate::save::same_version(
                    &before,
                    &target.metadata().unwrap()
                ));
            }
            COARSE_VERSION_PROBE.set(false);
        });
        assert_eq!(fs::read_to_string(target).unwrap(), first);
    }
}

#[test]
fn coarse_timestamps_do_not_lose_a_contended_same_size_save() {
    let scratch = tempfile::tempdir().unwrap();
    simultaneous_saves(&scratch.path().join("document"), true);
}

#[test]
fn coarse_timestamps_check_the_apps_digest_even_without_a_lock_wait() {
    let profile = tempfile::tempdir().unwrap();
    let path = profile.path().join("document");
    fs::write(&path, "original").unwrap();
    let before = path.metadata().unwrap();
    let disk = crate::content_identity(&path).unwrap();
    fs::write(&path, "accepted").unwrap();
    File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(before.modified().unwrap())
        .unwrap();
    COARSE_VERSION_PROBE.set(true);
    assert!(crate::save::same_version(
        &before,
        &path.metadata().unwrap()
    ));
    let result = save_report_with_expected(
        &path,
        &Text::from("rejected"),
        &Format::default(),
        false,
        Some(profile.path()),
        disk,
    );
    COARSE_VERSION_PROBE.set(false);
    assert!(
        result
            .unwrap_err()
            .contains("file changed while staging save")
    );
    assert_eq!(fs::read_to_string(&path).unwrap(), "accepted");
}
