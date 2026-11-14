use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use vel_build::{BuildSystem, Event, ResultParser, Task, Variables};
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
    let task = Task::spawn(&exec).unwrap();
    assert!(matches!(
        task.events.recv_timeout(Duration::from_secs(3)).unwrap(),
        Event::Output { .. }
    ));
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
