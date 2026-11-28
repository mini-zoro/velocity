#![cfg(unix)]

use std::{
    fs,
    os::unix::fs::{PermissionsExt, symlink},
    path::{Path, PathBuf},
    process::Command,
    sync::Arc,
    thread,
    time::{Duration, Instant},
};
use vel_index::{IndexOptions, ProjectIndex, sweep_stale};
use vel_search::project::Folder;
use vel_syntax::{MetadataSet, SyntaxSetBuilder};

/// Hang guard: stops a wait that would otherwise never end. It is not a latency
/// bound; the test checks the condition it waits for.
const HANG_GUARD: Duration = Duration::from_secs(120);

fn caches(root: &Path) -> Vec<PathBuf> {
    fs::read_dir(root)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.file_name()
                .unwrap()
                .as_encoded_bytes()
                .starts_with(b"velocity-index-")
        })
        .collect()
}

#[test]
fn temporary_index_child() {
    let Some(project) = std::env::var_os("VELOCITY_TEST_TEMPORARY_INDEX_PROJECT") else {
        return;
    };
    let temp = std::env::temp_dir();
    let mut syntaxes = SyntaxSetBuilder::new();
    syntaxes.add_plain_text_syntax();
    let index = ProjectIndex::open_folder_temporary(
        Folder {
            path: project.into(),
            name: String::new(),
            rules: Default::default(),
            follow_links: false,
        },
        Arc::new(syntaxes.build()),
        Arc::new(MetadataSet::default()),
        IndexOptions {
            watch: false,
            ..IndexOptions::default()
        },
    )
    .unwrap();
    index.flush().unwrap();
    let dirs = caches(&temp);
    assert_eq!(dirs.len(), 1);
    assert_eq!(dirs[0].metadata().unwrap().permissions().mode() & 0o077, 0);
    assert!(dirs[0].read_dir().unwrap().count() > 0);
    if std::env::var_os("VELOCITY_TEST_KEEP_INDEX_RUNNING").is_some() {
        // Exercise C exit (also used by AppKit), with no Rust destructor on this stack.
        std::process::exit(0);
    }
    index.cancel();
    let limit = Instant::now() + HANG_GUARD;
    while dirs[0].exists() {
        assert!(Instant::now() < limit, "cache remained after cancellation");
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn temporary_index_is_removed_on_shutdown_and_process_exit() {
    let fixture = tempfile::tempdir().unwrap();
    let project = fixture.path().join("project");
    let temp = fixture.path().join("runtime");
    fs::create_dir(&project).unwrap();
    fs::create_dir(&temp).unwrap();
    for exit_with_active_index in [false, true] {
        let mut child = Command::new(std::env::current_exe().unwrap());
        child
            .args(["--exact", "temporary_index_child", "--nocapture"])
            .env("TMPDIR", &temp)
            .env("VELOCITY_TEST_TEMPORARY_INDEX_PROJECT", &project)
            .env_remove("VELOCITY_TEST_KEEP_INDEX_RUNNING");
        if exit_with_active_index {
            child.env("VELOCITY_TEST_KEEP_INDEX_RUNNING", "1");
        }
        let output = child.output().unwrap();
        assert!(output.status.success(), "{output:?}");
        assert!(caches(&temp).is_empty(), "child left a cache directory");
    }
}

#[test]
fn stale_sweep_preserves_live_unrecognized_and_symlink_paths() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().join("runtime");
    let outside = fixture.path().join("outside");
    fs::create_dir(&root).unwrap();
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("keep"), "untouched").unwrap();
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "temporary_index_child"])
        .env_remove("VELOCITY_TEST_TEMPORARY_INDEX_PROJECT")
        .spawn()
        .unwrap();
    let stale = child.id();
    assert!(child.wait().unwrap().success());
    let live = std::process::id();
    let stale_primary = root.join(format!("velocity-index-{stale}"));
    let stale_fallback = root.join(format!("velocity-index-fallback-{stale}-0"));
    let keep = [
        root.join(format!("velocity-index-{live}")),
        root.join(format!("velocity-index-fallback-{live}-12")),
        root.join(format!("velocity-index-0{stale}")),
        root.join(format!("velocity-index-fallback-{stale}-0-extra")),
        root.join("velocity-index-0"),
    ];
    for path in keep.iter().chain([&stale_primary, &stale_fallback]) {
        fs::create_dir(path).unwrap();
    }
    symlink(&outside, stale_primary.join("outside")).unwrap();
    let linked = root.join(format!("velocity-index-fallback-{stale}-1"));
    symlink(&outside, &linked).unwrap();
    let file = root.join(format!("velocity-index-fallback-{stale}-2"));
    fs::write(&file, "not a directory").unwrap();
    sweep_stale(&root);
    assert!(!stale_primary.exists());
    assert!(!stale_fallback.exists());
    assert!(keep.iter().all(|path| path.is_dir()));
    assert!(fs::symlink_metadata(linked).unwrap().is_symlink());
    assert!(file.is_file());
    assert_eq!(
        fs::read_to_string(outside.join("keep")).unwrap(),
        "untouched"
    );
}
