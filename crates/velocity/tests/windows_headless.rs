#![cfg(windows)]
use std::{fs, process::Command};

#[path = "../src/cli.rs"]
#[allow(dead_code)]
mod cli;
#[path = "../src/instance_path.rs"]
mod instance_path;
#[path = "../src/instance/pipe.rs"]
#[allow(dead_code)]
mod instance_pipe;
#[path = "../src/instance_protocol.rs"]
#[allow(dead_code)]
mod instance_protocol;

#[test]
fn windows_executable_runs_headless_without_a_window() {
    let dir = tempfile::tempdir().unwrap();
    let profile = dir.path().join("roaming");
    let local = dir.path().join("local");
    let output = dir.path().join("smoke.jsonl");
    let result = Command::new(env!("CARGO_BIN_EXE_velocity"))
        .env("APPDATA", &profile)
        .env("LOCALAPPDATA", &local)
        .env("VELOCITY_LOG_DIR", dir.path().join("logs"))
        .env_remove("VELOCITY_DATA_DIR")
        .args(["--headless-smoke", "--lines", "10", "--output"])
        .arg(&output)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let report = fs::read_to_string(&output).unwrap();
    assert!(
        report.lines().any(|line| {
            serde_json::from_str::<serde_json::Value>(line)
                .is_ok_and(|record| record["complete"] == true)
        }),
        "{report}"
    );
}

#[test]
fn console_client_forwards_and_waits_for_edited_stdin() {
    use instance_pipe::Endpoint;
    use instance_protocol::{ENQUEUED, Request};
    use std::{
        io::{Read, Write},
        process::Stdio,
        sync::atomic::{AtomicBool, Ordering},
        time::{Duration, Instant},
    };

    let dir = tempfile::tempdir().unwrap();
    let helper = dir.path().join("vel.exe");
    fs::copy(env!("CARGO_BIN_EXE_vel"), &helper).unwrap();
    // No sibling GUI here: failure to connect cannot open a real window.
    let profile = dir.path().join("test Profile");
    let mut reply = Endpoint::for_profile(Some(&profile))
        .unwrap()
        .listen(true)
        .unwrap();
    let mut child = Command::new(helper)
        .arg("--data-dir")
        .arg(&profile)
        .arg("file.txt:12:3")
        .arg("-")
        .current_dir(dir.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all("original 你好".as_bytes())
        .unwrap();

    // Hang guard for every wait below; the client normally answers at once.
    let timeout = Duration::from_secs(120);
    let stop = AtomicBool::new(false);
    std::thread::scope(|scope| {
        let watchdog = scope.spawn(|| {
            std::thread::park_timeout(timeout);
            stop.store(true, Ordering::Release);
        });
        let result = reply.accept(&stop);
        watchdog.thread().unpark();
        result.unwrap();
    });
    reply.set_read_timeout(Some(timeout)).unwrap();
    reply.set_write_timeout(Some(timeout)).unwrap();
    let mut header = [0; 8];
    reply.read_exact(&mut header).unwrap();
    let len = usize::try_from(u64::from_le_bytes(header)).unwrap();
    assert!(len < 4096);
    let mut payload = vec![0; len];
    reply.read_exact(&mut payload).unwrap();
    let request: Request = serde_json::from_slice(&payload).unwrap();
    assert!(request.wait);
    assert_eq!(request.files[0].path, dir.path().join("file.txt"));
    assert_eq!((request.files[0].line, request.files[0].column), (12, 3));
    assert_eq!(
        request.stdin.unwrap().as_bytes(),
        "original 你好".as_bytes()
    );
    reply.write_all(&[ENQUEUED]).unwrap();
    assert!(child.try_wait().unwrap().is_none());
    let edited = "edited 你好".as_bytes();
    reply.write_all(&[2]).unwrap();
    reply
        .write_all(&(edited.len() as u64).to_le_bytes())
        .unwrap();
    reply.write_all(edited).unwrap();
    let until = Instant::now() + timeout;
    while child.try_wait().unwrap().is_none() {
        if Instant::now() >= until {
            let _ = child.kill();
            let _ = child.wait();
            panic!("vel.exe did not consume the close reply");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let result = child.wait_with_output().unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(result.stdout, edited);
}
