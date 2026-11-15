use crate::{cli::FileLocation, panels::Panels, view::View, workspace::Workspace};
use std::time::{Duration, Instant};
use vel_config::{json, settings::Layer};
use vel_text::Text;

fn wait(
    workspace: &mut Workspace,
    panels: &mut Panels,
    view: &mut View,
    ready: impl Fn(&Workspace, &Panels) -> bool,
) {
    let until = Instant::now() + Duration::from_secs(20);
    loop {
        std::hint::black_box(view.scene(workspace.editor()));
        workspace.poll();
        panels.poll(workspace, 0..100);
        if ready(workspace, panels) {
            return;
        }
        assert!(
            Instant::now() < until,
            "review UI timeout: {:?}",
            workspace.messages
        );
        std::thread::park_timeout(Duration::from_millis(1));
    }
}
fn setup(text: &str) -> (Workspace, Panels, View) {
    let mut workspace = Workspace::new(Text::from(text), None, None, false).unwrap();
    workspace
        .editor_mut()
        .settings
        .set(Layer::View, "auto_complete", json!(false));
    workspace
        .editor_mut()
        .settings
        .set(Layer::View, "word_wrap", json!(false));
    workspace.editor_mut().sync();
    let panels = Panels::new(&workspace, false);
    (
        workspace,
        panels,
        View::new(
            vel_platform::Size {
                width: 1200.,
                height: 800.,
            },
            7.8,
            12.,
        ),
    )
}
#[test]
fn diff_navigation_inline_geometry_revert_undo_and_snapshot_gap() {
    let (mut w, mut p, mut v) = setup("one\ntwo\nthree\n");
    w.editor_mut().set_caret(4, false);
    w.editor_mut()
        .run("insert", &json!({"characters":"new "}))
        .unwrap();
    wait(&mut w, &mut p, &mut v, |w, _| {
        w.editor()
            .annotations
            .borrow()
            .current
            .as_ref()
            .is_some_and(|p| p.hunks.len() == 1)
    });
    w.editor_mut()
        .run("toggle_inline_diff", &json!({}))
        .unwrap();
    let scene = v.scene(w.editor());
    assert!(
        scene
            .layers
            .iter()
            .flat_map(|l| &l.text)
            .any(|r| r.text.contains("two"))
    );
    assert_eq!(w.editor().layout.borrow().total(), 5);
    w.editor_mut()
        .run("move", &json!({"by":"lines","forward":true}))
        .unwrap();
    assert_eq!(w.editor().text.byte_to_line(w.editor().caret), 2);
    w.editor_mut()
        .run("move", &json!({"by":"lines","forward":false}))
        .unwrap();
    assert_eq!(w.editor().text.byte_to_line(w.editor().caret), 1);
    let at = w.editor().text.line_to_byte(2);
    let rect = v.rect_at(w.editor(), at);
    assert_eq!(
        v.hit_test(
            w.editor(),
            vel_platform::Point {
                x: rect.x,
                y: rect.y + 1.
            }
        ),
        at
    );
    w.editor_mut().run("revert_hunk", &json!({})).unwrap();
    assert_eq!(w.editor().text.to_string(), "one\ntwo\nthree\n");
    w.editor_mut().run("undo", &json!({})).unwrap();
    assert_eq!(w.editor().text.to_string(), "one\nnew two\nthree\n");
    // Undo publishes several descriptors without intermediate roots: reset from current.
    w.editor_mut()
        .run("insert", &json!({"characters":"a"}))
        .unwrap();
    w.editor_mut()
        .run("insert", &json!({"characters":"b"}))
        .unwrap();
    w.editor_mut().run("undo", &json!({})).unwrap();
    wait(&mut w, &mut p, &mut v, |w, _| {
        w.editor().annotations.borrow().pending == 0
    });
    let a = w.editor().annotations.borrow();
    let current = a.current.as_ref().unwrap();
    let exact = vel_diff::DocumentDiff::new(
        current.reference.clone(),
        w.editor().text.clone(),
        w.editor().buffer.change_id(),
    );
    assert_eq!(&*current.hunks, exact.hunks());
    drop(a);
    w.editor_mut().set_caret(0, false);
    w.editor_mut().run("next_modification", &json!({})).unwrap();
    assert_eq!(w.editor().text.byte_to_line(w.editor().caret), 1);
    w.editor_mut()
        .settings
        .set(Layer::View, "mini_diff", json!("auto"));
    w.editor_mut().sync();
    wait(&mut w, &mut p, &mut v, |w, _| {
        w.editor().annotations.borrow().pending == 0
    });
    assert!(
        w.editor()
            .annotations
            .borrow()
            .current
            .as_ref()
            .unwrap()
            .hunks
            .is_empty()
    );
}
#[test]
fn exact_scoped_spell_suggestions_context_replacement_ignore_and_offscreen_navigation() {
    let source = format!(
        "mispelled word\n{}another speling\n",
        "good word\n".repeat(150)
    );
    let (mut w, mut p, mut v) = setup(&source);
    w.editor_mut()
        .settings
        .set(Layer::View, "spell_check", json!(true));
    w.editor_mut()
        .settings
        .set(Layer::View, "spelling_selector", json!("text"));
    w.editor_mut().sync();
    wait(&mut w, &mut p, &mut v, |w, _| {
        w.editor()
            .annotations
            .borrow()
            .current
            .as_ref()
            .is_some_and(|a| !a.spelling.is_empty())
    });
    let at = w
        .editor()
        .annotations
        .borrow()
        .current
        .as_ref()
        .unwrap()
        .spelling[0]
        .range
        .start;
    p.review_context(&mut w, vel_platform::Point { x: 200., y: 150. }, at, None);
    assert!(
        p.review
            .items
            .iter()
            .any(|i| i.caption == "Add to Dictionary")
    );
    wait(&mut w, &mut p, &mut v, |_, p| {
        p.review
            .items
            .iter()
            .any(|i| i.invocation.command == "replace_misspelling")
    });
    let replacement = p
        .review
        .items
        .iter()
        .find(|i| i.invocation.command == "replace_misspelling")
        .unwrap()
        .invocation
        .clone();
    w.editor_mut()
        .run(&replacement.command, &replacement.args)
        .unwrap();
    assert!(!w.editor().text.to_string().starts_with("mispelled"));
    w.editor_mut().run("undo", &json!({})).unwrap();
    p.review.items.clear();
    p.review_action(&mut w, "navigate_misspelling", &json!({"forward":true}))
        .unwrap();
    wait(&mut w, &mut p, &mut v, |w, _| {
        w.editor().text.byte_to_line(w.editor().caret) > 100
    });
    assert_eq!(
        w.editor().text.slice(w.editor().selection()).to_string(),
        "speling"
    );
    w.editor_mut()
        .run("ignore_word", &json!({"word":"mispelled"}))
        .unwrap();
    w.editor_mut()
        .settings
        .set(Layer::View, "spelling_selector", json!("comment"));
    w.editor_mut().sync();
    wait(&mut w, &mut p, &mut v, |w, _| {
        w.editor().annotations.borrow().pending == 0
    });
    assert!(
        w.editor()
            .annotations
            .borrow()
            .current
            .as_ref()
            .unwrap()
            .spelling
            .is_empty()
    );
    // A saved suggestion cannot replace a later buffer version.
    w.editor_mut()
        .run("insert", &json!({"characters":"changed"}))
        .unwrap();
    let text = w.editor().text.clone();
    w.editor_mut()
        .run(&replacement.command, &replacement.args)
        .unwrap();
    assert_eq!(w.editor().text, text);
}
#[test]
fn git_badges_status_index_head_and_unconfigured_click_actions() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let path = root.join("file.rs");
    std::fs::write(&path, "old\n").unwrap();
    std::fs::write(root.join("missing.txt"), "missing\n").unwrap();
    std::fs::write(root.join(".gitignore"), "ignored/\n").unwrap();
    std::fs::create_dir(root.join("ignored")).unwrap();
    std::fs::write(root.join("ignored/file.txt"), "ignored\n").unwrap();
    crate::review_bench::repository(&root).unwrap();
    std::fs::remove_file(root.join("missing.txt")).unwrap();
    std::fs::write(&path, "index\n").unwrap();
    assert!(
        std::process::Command::new("git")
            .args(["add", "file.rs"])
            .current_dir(&root)
            .status()
            .unwrap()
            .success()
    );
    std::fs::write(&path, "working\n").unwrap();
    let (mut w, mut p, mut v) = setup("");
    w.open(FileLocation {
        path: path.clone(),
        line: 1,
        column: 1,
    })
    .unwrap();
    p.add_folder(&mut w, root.clone()).unwrap();
    wait(&mut w, &mut p, &mut v, |w, _| {
        w.git.snapshot(&path).is_some() && w.editor().annotations.borrow().current.is_some()
    });
    wait(&mut w, &mut p, &mut v, |_, p| {
        p.side_rows
            .iter()
            .any(|r| r.path == root.join("missing.txt"))
            && p.side_rows.iter().any(|r| r.path == root.join("ignored"))
    });
    assert_eq!(
        w.git.badge(&root.join("ignored"), true),
        Some(vel_git::Badge::Ignored)
    );
    assert_eq!(
        w.git.badge(&path, false),
        Some(vel_git::Badge::StagedModification)
    );
    assert_eq!(
        w.git.badge(&root, true),
        Some(vel_git::Badge::StagedModification)
    );
    assert!(w.git.status(&path).unwrap().contains("S1"));
    assert_eq!(
        w.editor()
            .annotations
            .borrow()
            .current
            .as_ref()
            .unwrap()
            .reference
            .to_string(),
        "index\n"
    );
    assert!(p.git_items(&w, &path, false).is_empty());
    w.editor_mut()
        .settings
        .set(Layer::View, "git_diff_target", json!("head"));
    w.editor_mut().sync();
    wait(&mut w, &mut p, &mut v, |w, _| {
        w.editor()
            .annotations
            .borrow()
            .current
            .as_ref()
            .is_some_and(|a| a.reference == "old\n")
    });
    w.editor_mut()
        .settings
        .set(Layer::View, "sublime_merge_path", json!("configured-tool"));
    assert_eq!(p.git_items(&w, &path, false).len(), 4);
    w.settings.set(Layer::View, "show_git_status", json!(false));
    w.poll();
    assert!(w.git.snapshot(&path).is_none());
    assert_eq!(w.git.badge(&path, false), None);
}
#[test]
fn dictionary_selection_and_persistent_words_keep_existing_user_settings() {
    let dir = tempfile::tempdir().unwrap();
    let user = dir.path().join("Packages/User");
    std::fs::create_dir_all(&user).unwrap();
    std::fs::write(user.join("custom.aff"), "SET UTF-8\n").unwrap();
    std::fs::write(user.join("custom.dic"), "1\ncustomword\n").unwrap();
    let preferences = user.join("Preferences.sublime-settings");
    std::fs::write(&preferences, "{// retained\n\"font_size\":17,}\n").unwrap();
    let mut w = Workspace::new(
        Text::from("customword alienword\n"),
        Some(dir.path().into()),
        None,
        false,
    )
    .unwrap();
    let mut p = Panels::new(&w, false);
    p.review_action(&mut w, "show_dictionary", &json!({}))
        .unwrap();
    assert!(
        p.quick
            .items
            .iter()
            .any(|i| i.caption.contains("custom.dic"))
            || p.overlay.is_some()
    );
    p.review_action(
        &mut w,
        "set_dictionary",
        &json!({"dictionary":"Packages/User/custom.dic"}),
    )
    .unwrap();
    w.editor_mut()
        .settings
        .set(Layer::View, "spell_check", json!(true));
    w.editor_mut().sync();
    let mut v = View::new(
        vel_platform::Size {
            width: 1200.,
            height: 800.,
        },
        7.8,
        12.,
    );
    wait(&mut w, &mut p, &mut v, |w, _| {
        w.editor()
            .annotations
            .borrow()
            .current
            .as_ref()
            .is_some_and(|p| p.spelling.len() == 1)
    });
    assert_eq!(
        w.editor()
            .annotations
            .borrow()
            .current
            .as_ref()
            .unwrap()
            .spelling[0]
            .word,
        "alienword"
    );
    p.review_action(
        &mut w,
        "persist_spelling_words",
        &json!({"key":"added_words","words":["alienword"]}),
    )
    .unwrap();
    let source = std::fs::read_to_string(preferences).unwrap();
    assert!(source.contains("// retained"));
    assert!(source.contains("\"font_size\":17"));
    assert!(source.contains("alienword"));
}

#[test]
fn review_benchmark_keys_undo_independently_of_prepared_prefix() {
    let options = crate::cli::Options {
        review_ui: Some(true),
        intel_words: 0,
        ..Default::default()
    };
    let mut prepared = crate::intelligence_bench::Prepared::new(&options).unwrap();
    assert!(prepared.view.size.width < options.width as f64);
    let before = prepared.workspace.editor().text.clone();
    for _ in 0..3 {
        prepared.wait_popup().unwrap();
        crate::type_through_keymap(prepared.workspace.editor_mut(), "0").unwrap();
        prepared.reset().unwrap();
        assert_eq!(prepared.workspace.editor().text, before);
    }
}

#[test]
fn cloned_viewports_keep_independent_spelling_requests_without_an_idle_work_loop() {
    let (mut w, mut p, mut v) = setup(&"mispelled word\n".repeat(150));
    w.editor_mut()
        .settings
        .set(Layer::View, "spell_check", json!(true));
    w.editor_mut().sync();
    w.clone_file().unwrap();
    let mut second = View::new(
        vel_platform::Size {
            width: 1200.,
            height: 800.,
        },
        7.8,
        12.,
    );
    second.scroll_y = 100. * second.line_height;
    let until = Instant::now() + Duration::from_secs(15);
    loop {
        w.prepare_sheet(0).unwrap();
        std::hint::black_box(v.scene(&w.tabs[0].editor));
        w.prepare_sheet(1).unwrap();
        std::hint::black_box(second.scene(&w.tabs[1].editor));
        w.poll();
        p.poll(&mut w, 0..150);
        let annotations = w.editor().annotations.borrow();
        if annotations.pending == 0
            && annotations.current.as_ref().is_some_and(|p| {
                p.spelling.iter().any(|m| m.range.start == 0)
                    && p.spelling.iter().any(|m| m.range.start > 1000)
            })
        {
            break;
        }
        assert!(Instant::now() < until);
        drop(annotations);
        std::thread::park_timeout(Duration::from_millis(1));
    }
    for _ in 0..3 {
        w.prepare_sheet(0).unwrap();
        std::hint::black_box(v.scene(&w.tabs[0].editor));
        w.prepare_sheet(1).unwrap();
        std::hint::black_box(second.scene(&w.tabs[1].editor));
    }
    assert_eq!(w.editor().annotations.borrow().pending, 0);
}

#[test]
fn git_reference_bom_and_crlf_normalize_before_diff_and_revert() {
    for utf16 in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let path = root.join("text.txt");
        let source = "hello 你好\r\nsecond\r\n";
        let bytes = if utf16 {
            [
                vec![0xff, 0xfe],
                source.encode_utf16().flat_map(u16::to_le_bytes).collect(),
            ]
            .concat()
        } else {
            [vec![0xef, 0xbb, 0xbf], source.as_bytes().to_vec()].concat()
        };
        std::fs::write(&path, bytes).unwrap();
        crate::review_bench::repository(&root).unwrap();
        let (mut w, mut p, mut v) = setup("");
        w.open(FileLocation {
            path: path.clone(),
            line: 1,
            column: 1,
        })
        .unwrap();
        wait(&mut w, &mut p, &mut v, |w, _| {
            w.editor().annotations.borrow().current.is_some()
        });
        assert!(
            w.editor()
                .annotations
                .borrow()
                .current
                .as_ref()
                .unwrap()
                .hunks
                .is_empty()
        );
        w.editor_mut()
            .run("insert", &json!({"characters":"changed "}))
            .unwrap();
        wait(&mut w, &mut p, &mut v, |w, _| {
            w.editor().annotations.borrow().pending == 0
        });
        w.editor_mut().run("revert_hunk", &json!({})).unwrap();
        assert_eq!(w.editor().text, "hello 你好\nsecond\n");
        w.save(w.active, None).unwrap();
        let loaded = vel_fileio::load(&path, "UTF-8", |_| {}).unwrap();
        assert_eq!(loaded.text, "hello 你好\nsecond\n");
        assert_eq!(loaded.format.line_ending, vel_fileio::LineEnding::CrLf);
    }
}
