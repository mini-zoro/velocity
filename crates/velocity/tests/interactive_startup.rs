#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

use serde_json::Value;
use std::{path::Path, process::Command};
use vel_session::{DiskStamp, Document, Session, Sheet, Window};

fn restore_fixture(root: &Path, profile: &Path) -> std::path::PathBuf {
    let mut session = Session::default();
    let mut window = Window::default();
    let mut active = None;
    for id in 1..=10 {
        let path = root.join(format!("tab{id}.c"));
        std::fs::write(&path, format!("int file_{id}(void) {{ return {id}; }}\n")).unwrap();
        session.documents.push(Document {
            id,
            path: Some(path.clone()),
            disk: DiskStamp::read(&path),
            disk_content: None,
            saved_in_effective_encoding: false,
            effective_encoding: None,
            encoding: "UTF-8".into(),
            encoding_chosen: false,
            preferred_encoding: None,
            encoding_display_name: None,
            bom: false,
            line_ending: "LF".into(),
            dirty: false,
            history: None,
        });
        window.sheets.push(Sheet {
            id,
            document: id,
            view: Default::default(),
            syntax: None,
            transient: false,
            html: None,
            settings: None,
        });
        window.workbench.groups[0].add(id, false);
        if id == 5 {
            active = Some(path);
        }
    }
    window.workbench.groups[0].click(5, false, false);
    session.windows.push(window);
    vel_session::save(&profile.join("Sessions/Session.sublime-session"), &session).unwrap();
    active.unwrap()
}

#[test]
fn first_frame_setup_is_bounded_with_an_empty_syntax_cache() {
    // Assert work, not time: scheduler load or parallel workspace tests cannot
    // turn waiting for a core into a failure. Rebuilding the stock YAML takes >10B.
    const BUDGET: u64 = 900_000_000;
    for restore in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let profile = root.path().join("profile");
        let home = root.path().join("home");
        let tmp = root.path().join("tmp");
        std::fs::create_dir(&home).unwrap();
        std::fs::create_dir(&tmp).unwrap();
        let active = restore.then(|| restore_fixture(root.path(), &profile));
        let result = root.path().join("startup.jsonl");
        let process = Command::new(env!("CARGO_BIN_EXE_velocity"))
            .args(["--headless-perf-startup-interactive", "--data-dir"])
            .arg(&profile)
            .arg("--output")
            .arg(&result)
            .env("HOME", &home)
            .env("SHELL", "/bin/zsh")
            .env("TMPDIR", &tmp)
            .env("VELOCITY_LOG_DIR", root.path().join("logs"))
            .output()
            .unwrap();
        assert!(
            process.status.success(),
            "{}",
            String::from_utf8_lossy(&process.stderr)
        );
        let records: Vec<Value> = std::fs::read_to_string(result)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        let phases: Vec<_> = records
            .iter()
            .filter(|r| r["kind"] == "startup_phase")
            .collect();
        assert_eq!(phases[0]["phase"], "interactive_setup_begin");
        assert_eq!(phases.last().unwrap()["phase"], "interactive_setup_ready");
        assert!(phases.iter().any(|r| r["phase"] == "menus_mousemap_ready"));
        assert!(!tmp.join("velocity-syntax-cache").exists());
        // Virtual machines (CI runners) expose no instruction counter; the phase order and
        // the other checks still run there.
        let instructions = |record: &Value| record["instructions"].as_u64();
        if !cfg!(debug_assertions)
            && let (Some(first), Some(last)) = (
                instructions(phases[0]),
                instructions(phases.last().unwrap()),
            )
        {
            let used = last - first;
            assert!((1..BUDGET).contains(&used), "restore={restore}: {used}");
        }
        let ready = records
            .iter()
            .find(|r| r["kind"] == "interactive_startup")
            .unwrap();
        assert_eq!(ready["complete"], true);
        assert_eq!(ready["gui_started"], false);
        assert_eq!(ready["gpu_started"], false);
        assert_eq!(ready["watcher_started"], false);
        assert_eq!(ready["palette_deferred"], true);
        if let Some(active) = active {
            assert_eq!(ready["tabs"], 10);
            assert_eq!(ready["lazy_tabs"], 9);
            assert_eq!(
                ready["active_path"],
                active.canonicalize().unwrap().to_str().unwrap()
            );
            assert_eq!(ready["syntax"], "Packages/C++/C.sublime-syntax");
        } else {
            assert_eq!(ready["lazy_tabs"], 0);
        }
    }
}
