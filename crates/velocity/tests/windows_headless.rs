#![cfg(windows)]
use std::{fs, process::Command};

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
