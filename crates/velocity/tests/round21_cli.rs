//! Exercise the real executable against a headless local peer; no GUI launch.
#![cfg(unix)]

#[path = "../src/instance_path.rs"]
mod instance_path;

use std::{
    fs::{self, DirBuilder},
    hash::{Hash, Hasher},
    io::{self, Read, Write},
    os::unix::{
        fs::DirBuilderExt,
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};

fn cli(home: &Path) -> Command {
    // A forwarding regression must fail the test without spawning AppKit.
    fs::write(home.join("profile"), "fixture: no native startup").unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_velocity"));
    command
        .env("HOME", home)
        .env("SHELL", "/bin/sh")
        .env("TMPDIR", home)
        .env("VELOCITY_LOG_DIR", home.join("logs"))
        .env("VELOCITY_INSTANCE_SOCKET_DIR", home.join("ipc"))
        .env_remove("VELOCITY_GUI_PROCESS")
        .env_remove("VELOCITY_GUI_STDERR")
        .env_remove("VELOCITY_INSTANCE_LOCK_FD")
        .arg("--data-dir")
        .arg(home.join("profile"))
        .stdin(Stdio::null());
    command
}

fn uid() -> u32 {
    // SAFETY: euid is an argument-free identity query.
    unsafe { libc::geteuid() }
}

fn legacy_path(home: &Path) -> PathBuf {
    // 5497418's original formula, independent of the current endpoint code.
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    Some(home.join("profile")).hash(&mut hasher);
    home.join(format!("velocity-{}-{:016x}.sock", uid(), hasher.finish()))
}

fn bind_fixture(home: &Path) -> UnixListener {
    let root = home.join("ipc");
    let path = instance_path::socket_path(Some(&home.join("profile")), uid());
    DirBuilder::new().mode(0o700).create(&root).unwrap();
    UnixListener::bind(root.join(path.file_name().unwrap())).unwrap()
}

fn receive(listener: &UnixListener) -> (UnixStream, Vec<u8>) {
    listener.set_nonblocking(true).unwrap();
    let until = std::time::Instant::now() + Duration::from_secs(5);
    let mut peer = loop {
        match listener.accept() {
            Ok((peer, _)) => break peer,
            Err(e)
                if e.kind() == io::ErrorKind::WouldBlock && std::time::Instant::now() < until =>
            {
                std::thread::sleep(Duration::from_millis(2));
            }
            Err(e) => panic!("CLI did not connect: {e}"),
        }
    };
    peer.set_nonblocking(false).unwrap();
    peer.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let mut len = [0; 8];
    peer.read_exact(&mut len).unwrap();
    let mut body = vec![0; u64::from_le_bytes(len) as usize];
    peer.read_exact(&mut body).unwrap();
    (peer, body)
}

#[test]
fn real_cli_wait_interrupt_is_client_only_and_literal_colon_wins() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    let data = root.join("profile");
    let listener = bind_fixture(root);
    let file = root.join("notes:2024");
    fs::write(&file, "fixture").unwrap();
    let client = cli(root)
        .args(["-w", "-s"])
        .arg(&file)
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let (mut peer, body) = receive(&listener);
    let request: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(request["files"][0]["path"], file.to_str().unwrap());
    assert_eq!(request["files"][0]["line"], 0, "bare path on the wire");
    assert_eq!(request["wait"], true);
    peer.write_all(&[1]).unwrap();
    // SAFETY: this sends SIGINT only to the child created by the test, not a group.
    assert_eq!(unsafe { libc::kill(client.id() as i32, libc::SIGINT) }, 0);
    let output = client.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(130));
    assert!(String::from_utf8_lossy(&output.stderr).contains("wait interrupted"));
    assert!(data.is_file());
    assert!(!root.join("logs").exists());
}

#[test]
fn real_cli_forwards_to_legacy_wire_without_claiming_the_profile() {
    #[derive(serde::Deserialize)]
    struct OldRequest {
        files: Vec<OldLocation>,
        stdin: Option<String>,
        wait: bool,
    }
    #[derive(serde::Deserialize)]
    struct OldLocation {
        path: PathBuf,
    }

    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    let legacy = legacy_path(root);
    let listener = UnixListener::bind(&legacy).unwrap();
    let file = root.join("notes.txt");
    fs::write(&file, "disk text").unwrap();
    let mut child = cli(root)
        .args(["-s", "-"])
        .arg(&file)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"terminal input")
        .unwrap();
    let (mut peer, bytes) = receive(&listener);
    let wire: OldRequest = serde_json::from_slice(&bytes).unwrap();
    assert!(wire.wait);
    assert_eq!(wire.files[0].path, file);
    assert_eq!(wire.stdin.as_deref(), Some("terminal input"));
    peer.write_all(&[1, 2]).unwrap();
    peer.write_all(&6_u64.to_le_bytes()).unwrap();
    peer.write_all(b"edited").unwrap();
    drop(peer);
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"edited");
    assert!(!root.join("logs").exists());
    drop(listener);
    let output = cli(root).arg(&file).output().unwrap();
    // The stale path was removed; the CLI reached the claim, which this fixture
    // deliberately blocks to ensure it cannot spawn a native window.
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("removed stale legacy socket"), "{stderr}");
    assert!(
        !stderr.contains("quit the running Velocity first"),
        "{stderr}"
    );
    assert!(!legacy.exists());
}

#[cfg(target_os = "macos")]
#[test]
fn real_cli_does_not_remove_a_live_legacy_socket_with_a_full_queue() {
    use std::os::{fd::AsRawFd, unix::fs::MetadataExt};

    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    let path = legacy_path(root);
    let listener = UnixListener::bind(&path).unwrap();
    let original = path.symlink_metadata().unwrap();
    // SAFETY: listener owns this descriptor; a short backlog makes the
    // Darwin ECONNREFUSED case reproducible without hundreds of clients.
    assert_eq!(unsafe { libc::listen(listener.as_raw_fd(), 1) }, 0);
    let mut pending = Vec::new();
    loop {
        match UnixStream::connect(&path) {
            Ok(stream) => pending.push(stream),
            Err(error) if error.kind() == io::ErrorKind::ConnectionRefused => break,
            Err(error) => panic!("unexpected connect error: {error}"),
        }
        assert!(
            pending.len() < 16,
            "the fixture's listen queue did not fill"
        );
    }
    let output = cli(root).arg("notes.txt").output().unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        output.stderr,
        b"velocity: Velocity is running but not responding; quit it first.\n"
    );
    let remaining = path.symlink_metadata().unwrap();
    assert_eq!(
        (original.dev(), original.ino()),
        (remaining.dev(), remaining.ino())
    );
    assert!(!root.join("logs").exists());
}

#[test]
fn real_legacy_wait_disconnect_only_reports_connection_closed() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    let listener = UnixListener::bind(legacy_path(root)).unwrap();
    let child = cli(root)
        .args(["-w", "-s", "notes.txt"])
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let (mut peer, _) = receive(&listener);
    peer.write_all(&[1]).unwrap();
    drop(peer);
    drop(listener);
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        output.stderr,
        b"velocity: connection to the editor closed\n"
    );
}

#[test]
fn real_nested_cli_with_leaked_gui_markers_forwards_and_reports_wait_eof() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    let listener = bind_fixture(root);
    let file = root.join("notes.txt");
    fs::write(&file, "disk text").unwrap();
    for fd in ["999999", "0", "3"] {
        let child = cli(root)
            .args(["-w", "-s"])
            .arg(&file)
            .env("VELOCITY_GUI_PROCESS", "1")
            .env("VELOCITY_GUI_STDERR", root.join("never-created.log"))
            .env("VELOCITY_INSTANCE_LOCK_FD", fd)
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let (mut peer, bytes) = receive(&listener);
        let request: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(request["wait"], true);
        peer.write_all(&[1]).unwrap();
        drop(peer);
        let output = child.wait_with_output().unwrap();
        assert_eq!(output.status.code(), Some(1));
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("connection to the editor closed"),
            "{stderr}"
        );
    }
    assert!(!root.join("never-created.log").exists());
    assert!(!root.join("logs").exists());
}

#[test]
fn real_help_version_and_benchmark_help_do_not_start_gui() {
    let fixture = tempfile::tempdir().unwrap();
    for (arg, needle) in [
        ("--help", "Usage:"),
        ("--version", "Velocity"),
        ("--help-bench", "--bench-startup"),
    ] {
        let result = cli(fixture.path()).arg(arg).output().unwrap();
        assert!(result.status.success(), "{arg}");
        let output = String::from_utf8(result.stdout).unwrap();
        assert!(output.contains(needle), "{output}");
        if arg == "--help" {
            for option in [
                "--new-window",
                "--launch-or-new-window",
                "--add",
                "--wait",
                "--background",
                "--stay",
                "--project",
                "--command",
                "--version",
                "--safe-mode",
            ] {
                assert!(output.contains(option), "{option}");
            }
            assert!(!output.contains("--bench"));
        }
    }
}
