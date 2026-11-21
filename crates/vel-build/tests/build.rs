mod support;
use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use vel_build::{BuildSystem, Event, ResultParser, Task, Variables};
#[cfg(unix)]
#[path = "support/detached.rs"]
mod detached_fixture;
#[test]
fn variables_nested_defaults_regex_unicode_and_literal_dollars() {
    let vars = Variables::new(
        Some(Path::new("/tmp/你好/a.test.rs")),
        None,
        None,
        Some(Path::new("/packages")),
    );
    assert_eq!(
        vars.expand("$file_name ${file_extension} ${folder:${project_path:${file_path}}}")
            .unwrap(),
        "a.test.rs rs /tmp/你好"
    );
    assert_eq!(
        vars.expand(r"${file_name/\.rs$/.exe/} \$HOME ${missing:hi}")
            .unwrap(),
        "a.test.exe $HOME hi"
    );
    assert_eq!(
        vars.expand("$project_name:$packages").unwrap(),
        ":/packages"
    );
    assert!(vars.expand("${file_name").is_err());
    assert!(vars.expand("${file/[bad/x/}").is_err());
}
#[test]
fn variant_environment_automatic_selection_and_custom_target_rejection() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("Cargo.toml"), "").unwrap();
    let source = r#"{"cmd":["cargo","build"],"keyfiles":["Cargo.toml"],"env":{"NAME":"$file_base_name"},"variants":[{"name":"Test","cmd":["cargo","test"]}],}"#;
    let build = BuildSystem::parse("Cargo", source).unwrap();
    assert!(build.matches("text.plain", None, &[dir.path().into()]));
    assert_eq!(build.variants(), vec!["Test"]);
    let exec = build
        .variant("Test")
        .unwrap()
        .resolve(&Variables::new(
            Some(Path::new("/tmp/a.rs")),
            None,
            None,
            None,
        ))
        .unwrap();
    assert_eq!(exec.cmd, vec!["cargo", "test"]);
    assert_eq!(exec.env["NAME"], "a");
    assert!(
        BuildSystem::parse("bad", r#"{"cmd":["true"],"target":"python_plugin"}"#)
            .unwrap()
            .resolve(&Variables::default())
            .is_err()
    );
}
#[test]
fn relative_file_regex_and_line_only_results_use_latest_header() {
    let mut parser = ResultParser::new(
        Some(r"^([^:]+)(?::(\d+):(\d+): (.*))?$"),
        Some(r"^line (\d+) col (\d+): (.*)"),
        PathBuf::from("/project"),
    )
    .unwrap();
    let d = parser.parse("src/a.rs:12:7: bad").unwrap();
    assert_eq!(d.path, PathBuf::from("/project/src/a.rs"));
    assert_eq!((d.line, d.column), (12, 7));
    assert_eq!(d.message, "bad");
    parser.parse("src/next.rs");
    let d = parser.parse("line 3 col 2: wrong").unwrap();
    assert_eq!(d.path, PathBuf::from("/project/src/next.rs"));
    assert_eq!((d.line, d.column), (3, 2));
}
#[test]
fn streams_both_pipes_and_preserves_split_utf8() {
    let build=BuildSystem::parse("fixture",r#"{"shell_cmd":"printf 'hello\\n'; printf 'error\\n' >&2; printf '\\344'; printf '\\275\\240\\n'"}"#).unwrap();
    let exec = build.resolve(&Variables::default()).unwrap();
    support::install();
    let task = Task::spawn(&exec).unwrap();
    let mut output = String::new();
    let mut stderr = false;
    loop {
        match task.events.recv_timeout(Duration::from_secs(3)).unwrap() {
            Event::Output { stream, text } => {
                stderr |= stream == vel_build::Stream::Stderr;
                output.push_str(&text);
            }
            Event::Finished {
                code, cancelled, ..
            } => {
                assert_eq!(code, Some(0));
                assert!(!cancelled);
                break;
            }
            Event::Error(e) => panic!("{e}"),
            Event::Warning(_)
            | Event::Truncated { .. }
            | Event::OutputBatch(_)
            | Event::Results(_)
            | Event::InputClosed => {}
        }
    }
    assert!(stderr);
    assert!(output.contains("hello"));
    assert!(output.contains("你"));
}
#[test]
fn cancel_reaps_process_tree_and_unblocks_inherited_pipes() {
    let build =
        BuildSystem::parse("fixture", r#"{"shell_cmd":"sleep 30 & echo ready; wait"}"#).unwrap();
    let exec = build.resolve(&Variables::default()).unwrap();
    support::install();
    let task = Task::spawn(&exec).unwrap();
    // Login fallback may report a warning before the process produces output.
    // Cancel only once the real child has started, not after an environment event.
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut output = String::new();
    loop {
        match task
            .events
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .unwrap()
        {
            Event::Warning(_)
            | Event::Truncated { .. }
            | Event::OutputBatch(_)
            | Event::Results(_)
            | Event::InputClosed => {}
            Event::Output { text, .. } => {
                output.push_str(&text);
                if output.contains("ready") {
                    break;
                }
            }
            event => panic!("child did not become ready: {event:?}"),
        }
    }
    let at = Instant::now();
    task.cancel();
    loop {
        if let Event::Finished { cancelled, .. } =
            task.events.recv_timeout(Duration::from_secs(3)).unwrap()
        {
            assert!(cancelled);
            break;
        }
    }
    assert!(at.elapsed() < Duration::from_secs(2));
}

#[test]
fn dropping_a_build_with_full_output_queue_reaps_its_tree_without_a_consumer() {
    let exec = BuildSystem::parse("output", r#"{"cmd":["/usr/bin/yes","output"]}"#)
        .unwrap()
        .resolve(&Variables::default())
        .unwrap();
    support::install();
    let task = Task::spawn(&exec).unwrap();
    std::thread::sleep(Duration::from_millis(30));
    let (done, completion) = std::sync::mpsc::channel();
    let at = Instant::now();
    let thread = std::thread::spawn(move || {
        drop(task);
        done.send(()).unwrap();
    });
    completion
        .recv_timeout(Duration::from_secs(3))
        .expect("Drop must reap even while its output queue is full");
    thread.join().unwrap();
    assert!(at.elapsed() < Duration::from_secs(2));
}

#[test]
fn round18_environment_path_working_directory_and_wrap_table() {
    let variables = Variables::new(Some(Path::new("/tmp/project/main.rs")), None, None, None);
    for (options, path_suffix, name) in [
        (
            r#"{"cmd":["true"],"env":{"NAME":"$file_base_name"}}"#,
            None,
            "main",
        ),
        (
            r#"{"cmd":["true"],"path":"/opt/bin:$PATH","env":{"NAME":"${file_name}","PATH":"ignored"}}"#,
            Some("/opt/bin:"),
            "main.rs",
        ),
    ] {
        let exec = BuildSystem::parse("env", options)
            .unwrap()
            .resolve(&variables)
            .unwrap();
        assert_eq!(exec.working_dir, Path::new("/tmp/project"));
        assert_eq!(exec.env["NAME"], name);
        if let Some(prefix) = path_suffix {
            assert_eq!(
                exec.env["PATH"],
                format!("{prefix}{}", std::env::var("PATH").unwrap_or_default())
            );
        }
        assert!(
            exec.word_wrap,
            "round-18 build output contract defaults to wrapping"
        );
    }
    for wrap in [false, true] {
        let build = BuildSystem::from_value(
            "wrap",
            serde_json::json!({"cmd":["true"],"word_wrap":wrap,"working_dir":"${file_path}/src"}),
        )
        .unwrap();
        let exec = build.resolve(&variables).unwrap();
        assert_eq!(exec.word_wrap, wrap);
        assert_eq!(exec.working_dir, Path::new("/tmp/project/src"));
    }
}
#[test]
fn round18_snippet_transform_first_global_captures_case_and_empty_matches() {
    let variables = Variables([(String::from("value"), String::from("aba"))].into());
    for (source, expected) in [
        ("${value/a/x/}", "xba"),
        ("${value/a/x/g}", "xbx"),
        (r"${value/(a)/\U$1/}", "Aba"),
        (r"${value/(a)/\U$1/g}", "AbA"),
        ("${value/(?=a)/x/g}", "xabxa"),
    ] {
        assert_eq!(variables.expand(source).unwrap(), expected, "{source}");
    }
}
#[test]
fn round18_selector_scoring_table_including_embedded_and_excluded_scopes() {
    let scopes = [
        "text.html.basic",
        "meta.tag",
        "source.js",
        "string.quoted.double.js",
    ]
    .map(|s| vel_scope::Scope::new(s).unwrap());
    let score = |selector: &str| {
        BuildSystem::from_value(
            "selector",
            serde_json::json!({"cmd":["true"],"selector":selector}),
        )
        .unwrap()
        .match_score(&scopes, None, &[])
    };
    for (selector, matched) in [
        ("source.js", true),
        ("source.python | source.js", true),
        ("source.js - comment", true),
        ("source.js - string", false),
        ("text.html source.js string", true),
        ("text.html > source.js", false),
        ("source.python", false),
    ] {
        assert_eq!(score(selector).is_some(), matched, "{selector}");
    }
    assert!(score("source.js string.quoted.double.js") > score("source.js"));
}
#[cfg(unix)]
#[test]
fn round18_restart_kills_child_and_grandchild_before_new_process_starts() {
    let dir = tempfile::tempdir().unwrap();
    let child_path = dir.path().join("child");
    let grandchild_path = dir.path().join("grandchild");
    let source = format!(
        "/bin/sh -c 'sleep 30 & echo $! > {}; wait' & echo $! > {}; echo ready; wait",
        grandchild_path.display(),
        child_path.display()
    );
    let exec = BuildSystem::from_value("old", serde_json::json!({"cmd":["/bin/bash","-c",source]}))
        .unwrap()
        .resolve(&Variables::default())
        .unwrap();
    support::install();
    let old = Task::spawn(&exec).unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    while !grandchild_path.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    let child: i32 = std::fs::read_to_string(&child_path)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let grandchild: i32 = std::fs::read_to_string(&grandchild_path)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let next = BuildSystem::parse("next", r#"{"cmd":["/bin/echo","replacement"]}"#)
        .unwrap()
        .resolve(&Variables::default())
        .unwrap();
    let at = Instant::now();
    support::install();
    let task = Task::spawn_after(&next, Some(old)).unwrap();
    assert!(
        at.elapsed() < Duration::from_millis(100),
        "restart must not wait on the caller"
    );
    let mut output = String::new();
    loop {
        match task.events.recv_timeout(Duration::from_secs(3)).unwrap() {
            Event::Output { text, .. } => output.push_str(&text),
            Event::Finished {
                code, cancelled, ..
            } => {
                assert_eq!(code, Some(0));
                assert!(!cancelled);
                break;
            }
            Event::Error(error) => panic!("{error}"),
            Event::Warning(_)
            | Event::Truncated { .. }
            | Event::OutputBatch(_)
            | Event::InputClosed
            | Event::Results(_) => {}
        }
    }
    assert_eq!(output, "replacement\n");
    for pid in [child, grandchild] {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            // SAFETY: signal zero probes only this fixture's known child PID.
            if unsafe { libc::kill(pid, 0) } != 0 {
                break;
            }
            let status = std::process::Command::new("/bin/ps")
                .args(["-p", &pid.to_string(), "-o", "stat="])
                .output()
                .unwrap();
            if String::from_utf8_lossy(&status.stdout)
                .trim()
                .starts_with('Z')
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "descendant {pid} is still running"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

#[test]
fn round18_variant_overrides_effective_platform_base_and_can_clear_shell_cmd() {
    let build=BuildSystem::parse("variant",r#"{"cmd":["base"],"shell_cmd":"base shell","osx":{"cmd":["platform"]},"linux":{"cmd":["platform"]},"windows":{"cmd":["platform"]},"variants":[{"name":"Run","cmd":["variant"],"shell_cmd":null}]}"#).unwrap();
    let exec = build
        .variant("Run")
        .unwrap()
        .resolve(&Variables::default())
        .unwrap();
    assert_eq!(exec.cmd, vec!["variant"]);
    assert!(exec.shell_cmd.is_none());
}

#[cfg(unix)]
#[test]
fn round18_detached_output_pipe_finishes_cancels_and_restarts_without_waiting() {
    for cancel in [false, true] {
        let fixture = detached_fixture::DetachedFixture::new();
        let script = fixture.script(false, "sleep .1; echo started");
        let exec = BuildSystem::from_value(
            "detached",
            serde_json::json!({"cmd":["/bin/sh","-c",script]}),
        )
        .unwrap()
        .resolve(&Variables::default())
        .unwrap();
        support::install();
        let old = Task::spawn(&exec).unwrap();
        loop {
            if matches!(
                old.events.recv_timeout(Duration::from_secs(3)).unwrap(),
                Event::Output { .. }
            ) {
                break;
            }
        }
        assert!(fixture.root().join("ready").exists());
        let at = Instant::now();
        if cancel {
            old.cancel();
        }
        loop {
            if let Event::Finished { cancelled, .. } =
                old.events.recv_timeout(Duration::from_secs(1)).unwrap()
            {
                assert_eq!(cancelled, cancel);
                break;
            }
        }
        assert!(at.elapsed() < Duration::from_secs(1));
        let replacement =
            BuildSystem::from_value("new", serde_json::json!({"cmd":["/bin/echo","NEW"]}))
                .unwrap()
                .resolve(&Variables::default())
                .unwrap();
        let at = Instant::now();
        let new = Task::spawn_after(&replacement, Some(old)).unwrap();
        let mut output = String::new();
        loop {
            match new.events.recv_timeout(Duration::from_secs(1)).unwrap() {
                Event::Output { text, .. } => output.push_str(&text),
                Event::Finished { .. } => break,
                _ => {}
            }
        }
        assert_eq!(output, "NEW\n");
        assert!(at.elapsed() < Duration::from_secs(1));
        // Lease removal releases the detached process without a signal.
        std::fs::remove_file(fixture.root().join("lease")).unwrap();
        let released = Instant::now();
        while !fixture.root().join("done").exists() {
            assert!(released.elapsed() < Duration::from_secs(3));
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    // Restart while the first worker is still in its pipe grace period.
    let old_exec = BuildSystem::from_value(
        "old",
        serde_json::json!({"cmd":["/bin/sh","-c","sleep 30 & echo OLD; wait"]}),
    )
    .unwrap()
    .resolve(&Variables::default())
    .unwrap();
    support::install();
    let old = Task::spawn(&old_exec).unwrap();
    loop {
        if matches!(
            old.events.recv_timeout(Duration::from_secs(3)).unwrap(),
            Event::Output { .. }
        ) {
            break;
        }
    }
    let new_exec =
        BuildSystem::from_value("new", serde_json::json!({"cmd":["/bin/echo","RESTART"]}))
            .unwrap()
            .resolve(&Variables::default())
            .unwrap();
    let at = Instant::now();
    let new = Task::spawn_after(&new_exec, Some(old)).unwrap();
    loop {
        if let Event::Output { text, .. } = new.events.recv_timeout(Duration::from_secs(1)).unwrap()
        {
            assert_eq!(text, "RESTART\n");
            break;
        }
    }
    assert!(at.elapsed() < Duration::from_secs(1));
}

#[test]
fn round18_third_task_defaults_cannot_capture_real_login_profiles() {
    let exec = BuildSystem::from_value("fixture", serde_json::json!({"cmd":["/usr/bin/env"]}))
        .unwrap()
        .resolve(&Variables::default())
        .unwrap();
    support::install();
    let task = Task::spawn(&exec).unwrap();
    let mut output = String::new();
    loop {
        match task.events.recv_timeout(Duration::from_secs(10)).unwrap() {
            Event::Output { text, .. } => output.push_str(&text),
            Event::Finished { .. } => break,
            Event::Error(e) => panic!("{e}"),
            _ => {}
        }
    }
    let env = output
        .lines()
        .filter_map(|s| s.split_once('='))
        .collect::<std::collections::BTreeMap<_, _>>();
    let home = Path::new(env["HOME"]);
    let shell = Path::new(env["SHELL"]);
    assert_eq!(shell.parent(), Some(home));
    assert!(shell.is_file());
    assert_ne!(home, Path::new(&std::env::var("HOME").unwrap()));
    assert!(
        std::fs::read_to_string(shell)
            .unwrap()
            .contains("exec /bin/sh -c")
    );
}

#[test]
fn round19_sustained_output_keeps_ui_polling_progress_with_regex() {
    support::install();
    // Full-speed 4 KiB diagnostic lines for three seconds; the same reader,
    // bounded transport and result snapshots as a user's running build.
    let source = r#"use Time::HiRes qw(time); $|=1; my $end=time()+3; my $line='fixture.rs:1:1: '.('x' x 4079)."\n"; while(time()<$end) { print $line; } print "TAIL_MARKER\n";"#;
    let exec = BuildSystem::from_value(
        "steady",
        serde_json::json!({
            "cmd":["/usr/bin/perl","-e",source.replace('$',r"\$")],
            "file_regex":"^([^:]+):([0-9]+):([0-9]+): (.*)$"
        }),
    )
    .unwrap()
    .resolve(&Variables::default())
    .unwrap();
    let task = Task::spawn(&exec).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut started = None;
    let mut last = Instant::now();
    let mut polls = 0;
    let mut updates = 0;
    let mut longest = Duration::ZERO;
    let mut sleep_stall = Duration::ZERO;
    let mut excluded_sleep = Duration::ZERO;
    let mut tail = String::new();
    let mut done = false;
    let mut errors = 0;
    while !done {
        assert!(Instant::now() < deadline);
        let cadence = Duration::from_millis(16);
        let sleeping = Instant::now();
        std::thread::sleep(cadence);
        // A delayed test thread did not poll the transport. Exclude only sleep
        // beyond the requested cadence; poll work and missed data polls count.
        let stall = sleeping.elapsed().saturating_sub(cadence);
        sleep_stall += stall;
        excluded_sleep += stall;
        let mut data = false;
        for _ in 0..128 {
            match task.events.try_recv() {
                Ok(Event::Output { text, .. }) => {
                    data = true;
                    tail = text;
                }
                Ok(Event::OutputBatch(chunks)) => {
                    data = true;
                    tail = chunks.into_iter().map(|(_, t)| t).collect();
                }
                Ok(Event::Results(results)) => errors = results.errors,
                Ok(Event::Finished {
                    code, cancelled, ..
                }) => {
                    assert_eq!(code, Some(0));
                    assert!(!cancelled);
                    done = true;
                    break;
                }
                Ok(Event::Error(e)) => panic!("{e}"),
                Ok(_) => {}
                Err(_) => break,
            }
        }
        let now = Instant::now();
        if started.is_some() && !done {
            polls += 1;
        }
        if data {
            if started.is_some() && !done {
                updates += 1;
                longest = longest.max((now - last).saturating_sub(sleep_stall));
            }
            started.get_or_insert(now);
            last = now;
            sleep_stall = Duration::ZERO;
        }
        let _ = task.events.results(usize::MAX);
    }
    eprintln!(
        "steady output: {updates}/{polls} data polls; UI gap {longest:?}; excluded sleep {excluded_sleep:?}; {errors} diagnostics"
    );
    assert!(polls >= 100);
    assert!(updates * 100 >= polls * 90);
    assert!(longest < Duration::from_millis(50));
    assert!(tail.ends_with("TAIL_MARKER\n"));
    assert!(errors > 0);
    let stats = task.events.stats();
    assert!(stats.peak_bytes <= 8 * 1024 * 1024);
    assert_eq!(stats.spill_file_bytes, 0);
}
