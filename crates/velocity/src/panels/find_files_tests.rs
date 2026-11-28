//! Find in Files through real workers, loaders and panel key bindings.
use super::*;
use crate::panels::{STATUS, status_position};
use vel_render::scene::Scene;
use vel_search::files::Row;

fn wait_for(a: &mut App, ready: impl Fn(&App) -> bool) {
    let deadline = Instant::now() + crate::test_timing::HANG_GUARD;
    loop {
        a.poll_workspace_model();
        a.panels.poll(&mut a.workspace, 0..0);
        if ready(a) {
            break;
        }
        assert!(Instant::now() < deadline, "{}", a.panels.message);
        std::thread::park_timeout(Duration::from_millis(2));
    }
}

fn begin(a: &mut App, pattern: &str) {
    named(
        a,
        "show_panel",
        json!({"panel":"find_in_files", "pattern":pattern}),
    );
    key(a, "enter");
}

#[test]
fn find_and_replace_in_files_use_the_editor_shift_jis_fallback() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let path = root.join("legacy.txt");
    let profile = tempfile::tempdir().unwrap();
    let fallback = "Japanese (Shift JIS)";
    let format = vel_fileio::encoding_format(fallback, vel_fileio::LineEnding::CrLf).unwrap();
    let encode = |text: &str| {
        let mut bytes = Vec::new();
        vel_fileio::encode_to(&mut bytes, &vel_text::Text::from(text), &format).unwrap();
        bytes
    };
    std::fs::write(&path, encode("前置き\n日本語 one\n終わり\n")).unwrap();
    let user = profile.path().join("Packages/User");
    std::fs::create_dir_all(&user).unwrap();
    std::fs::write(
        user.join("Preferences.sublime-settings"),
        json!({"fallback_encoding": fallback, "index_files": false}).to_string(),
    )
    .unwrap();
    let mut a = app("", "Plain Text", Some(profile.path()));
    add(&mut a, &root);
    begin(&mut a, "one");
    wait_for(&mut a, |a| a.panels.file_complete);
    let hit = a
        .panels
        .result_view
        .rows
        .iter()
        .find_map(|row| match row {
            Row::Line(l) if l.match_count != 0 => Some(l),
            _ => None,
        })
        .unwrap();
    assert_eq!(
        (hit.number, hit.column(), hit.text.as_str()),
        (2, 5, "日本語 one")
    );
    key(&mut a, "f4");
    wait_for(&mut a, |a| {
        a.workspace.tabs[a.workspace.active].loading.is_none()
    });
    assert_eq!(a.workspace.editor().caret, "前置き\n日本語 ".len());
    named(
        &mut a,
        "show_panel",
        json!({"panel":"find_in_files","pattern":"one","replace_pattern":"変更"}),
    );
    named(&mut a, "replace_in_files", json!({}));
    wait_for(&mut a, |a| a.panels.replacement_plan.is_some());
    a.confirm_file_replacements(|matches, files| {
        assert_eq!((matches, files), (1, 1));
        true
    });
    assert_eq!(
        std::fs::read(&path).unwrap(),
        encode("前置き\n日本語 変更\n終わり\n")
    );
    assert_eq!(
        a.workspace.editor().text.to_string(),
        "前置き\n日本語 変更\n終わり\n"
    );
}

#[test]
fn single_line_search_decodes_legacy_context_before_f4() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let path = root.join("legacy.txt");
    std::fs::write(&path, b"caf\xe9\npr\xe9 one\nnext\n").unwrap();
    let mut a = app("", "Plain Text", None);
    add(&mut a, &root);
    begin(&mut a, "one");
    wait_for(&mut a, |a| !a.panels.busy());
    let rows: Vec<_> = a
        .panels
        .result_view
        .rows
        .iter()
        .filter_map(|row| match row {
            Row::Line(l) => Some((l.number, l.text.as_str(), l.match_count, l.column())),
            _ => None,
        })
        .collect();
    assert_eq!(
        rows,
        [(1, "café", 0, 1), (2, "pré one", 1, 5), (3, "next", 0, 1)]
    );
    key(&mut a, "f4");
    wait_for(&mut a, |a| {
        a.workspace.tabs[a.workspace.active].loading.is_none()
    });
    let e = a.workspace.editor();
    assert_eq!(e.caret, e.text.line_to_byte(1) + "pré ".len());
}

#[test]
fn single_line_search_f4_counts_lone_cr_like_the_editor() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    std::fs::write(root.join("mixed.txt"), b"first\r\nsecond\ronly one\rlast\n").unwrap();
    let mut a = app("", "Plain Text", None);
    add(&mut a, &root);
    begin(&mut a, "one");
    wait_for(&mut a, |a| !a.panels.busy());
    let hit = a.panels.result_view.rows.iter().find_map(|row| match row {
        Row::Line(l) if l.match_count > 0 => Some((l.number, l.column())),
        _ => None,
    });
    assert_eq!(hit, Some((3, 6)));
    key(&mut a, "f4");
    wait_for(&mut a, |a| {
        a.workspace.tabs[a.workspace.active].loading.is_none()
    });
    let e = a.workspace.editor();
    assert_eq!(e.text.to_string(), "first\nsecond\nonly one\nlast\n");
    assert_eq!(e.caret, e.text.line_to_byte(2) + 5);
}

fn add(a: &mut App, root: &Path) {
    named(a, "add_folder", json!({"dir":root}));
}

#[test]
fn find_in_files_matches_clipboard_newline_on_disk() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    std::fs::write(root.join("match.txt"), b"before\r\none\r\ntwo\r\nafter\r\n").unwrap();
    std::fs::write(root.join("other.txt"), "one two\n").unwrap();
    let profile = tempfile::tempdir().unwrap();
    let mut a = app("", "Plain Text", Some(profile.path()));
    set(&mut a, "index_files", json!(false));
    add(&mut a, &root);
    key(&mut a, "super+shift+f");
    a.workspace.editor_mut().clipboard = "one\ntwo".into();
    key(&mut a, "super+v");
    assert_eq!(a.panels.query.value(), "one\ntwo");
    key(&mut a, "enter");
    wait_for(&mut a, |a| !a.panels.busy());
    let results: Vec<_> = a
        .panels
        .result_view
        .rows
        .iter()
        .filter_map(|row| match row {
            Row::Line(line) if !line.highlights.is_empty() => {
                Some((line.path.as_ref(), line.number))
            }
            _ => None,
        })
        .collect();
    assert_eq!(results, [(root.join("match.txt").as_path(), 2)]);
}

#[test]
fn disk_search_streams_cross_line_regex_and_skips_binary_without_feedback() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    // 'one' ends at the end of the first matcher window, and the match must
    // also survive the source's CRLF normalization.
    let mut text = "x".repeat(65536 - 4);
    text.push_str("\none\r\ntwo\r\nrest\n");
    std::fs::write(root.join("match.txt"), text).unwrap();
    std::fs::write(root.join("wrong.txt"), "one\nafraid\n").unwrap();
    std::fs::write(root.join("binary.dat"), b"one\ntwo\0binary").unwrap();
    std::fs::write(root.join("archive.bin"), b"\x7fELF\0one\ntwo\n").unwrap();
    let mut a = app("", "Plain Text", None);
    add(&mut a, &root);
    named(&mut a, "toggle_find_option", json!({"option":"regex"}));
    for (query, files) in [
        (r"one\ntwo", 1),
        (r"one\stwo", 1),
        ("(?s:one.*?two)", 1),
        ("one$", 2),
    ] {
        begin(&mut a, query);
        let deadline = Instant::now() + crate::test_timing::HANG_GUARD;
        while a.panels.busy() {
            let viewport = a.viewport();
            a.panels.poll(&mut a.workspace, viewport);
            assert!(
                !a.panels.message.contains("binary file"),
                "{}",
                a.panels.message
            );
            assert!(Instant::now() < deadline);
            std::thread::park_timeout(Duration::from_millis(1));
        }
        let hits: Vec<_> = a
            .panels
            .result_view
            .rows
            .iter()
            .filter_map(|r| match r {
                vel_search::files::Row::Line(l) if l.match_count > 0 => Some(l),
                _ => None,
            })
            .collect();
        assert_eq!(hits.len(), files, "{query:?}: {}", a.panels.message);
        assert!(
            hits.iter()
                .any(|l| l.path.ends_with("match.txt") && l.number == 2 && l.column() == 1)
        );
        assert!(
            hits.iter()
                .all(|l| l.path.ends_with("match.txt") || l.path.ends_with("wrong.txt"))
        );
    }
}

#[test]
fn multiline_search_places_eof_on_the_last_real_line() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let unterminated = root.join("last.txt");
    let terminated = root.join("lf.txt");
    std::fs::write(&unterminated, "first\nlast").unwrap();
    std::fs::write(&terminated, "first\nlast\n").unwrap();
    let mut a = app("", "Plain Text", None);
    add(&mut a, &root);
    named(&mut a, "toggle_find_option", json!({"option":"regex"}));
    begin(&mut a, r"(?:no\nsuch)|\z");
    wait_for(&mut a, |a| !a.panels.busy());
    let hits: Vec<_> = a
        .panels
        .result_view
        .rows
        .iter()
        .filter_map(|row| match row {
            Row::Line(line) if line.match_count > 0 => Some((
                line.path.as_ref(),
                line.number,
                line.column(),
                line.text.as_str(),
            )),
            _ => None,
        })
        .collect();
    assert_eq!(
        hits,
        [
            (unterminated.as_path(), 2, 5, "last"),
            (terminated.as_path(), 3, 1, "")
        ]
    );
}

#[test]
fn ordered_results_keep_row_zero_until_navigation_and_scale_across_files() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let first = root.join("first");
    let last = root.join("last");
    for folder in [&first, &last] {
        std::fs::create_dir(folder).unwrap();
        // More rows than the old block capacity; gaps make row rank distinct
        // from file rank.
        for i in 0..160 {
            std::fs::write(folder.join(format!("file{i:03}.txt")), "hit\n\n\n\n\nhit\n").unwrap();
        }
    }
    let profile = tempfile::tempdir().unwrap();
    let mut a = app("", "Plain Text", Some(profile.path()));
    add(&mut a, &first);
    add(&mut a, &last);
    named(
        &mut a,
        "show_panel",
        json!({"panel":"find_in_files","pattern":"hit"}),
    );
    key(&mut a, "tab");
    key(&mut a, "tab");
    key(&mut a, "super+a");
    input(&mut a, &format!("{},{}", last.display(), first.display()));
    key(&mut a, "enter");
    wait_for(&mut a, |a| !a.panels.busy());
    assert_eq!(a.panels.result_selected, 0);
    let rows = &a.panels.result_view.rows;
    assert_eq!(
        rows.iter()
            .filter(|r| matches!(r, vel_search::files::Row::Header(_)))
            .count(),
        320
    );
    assert_eq!(
        rows.get(0),
        Some(vel_search::files::Row::Header(&first.join("file000.txt")))
    );
    assert_eq!(rows.get(rows.len() - 1), rows.iter().next_back());
    for (i, row) in rows.iter().enumerate() {
        assert_eq!(rows.get(i), Some(row));
    }
    key(&mut a, "f4");
    assert_eq!(
        a.workspace.tabs[a.workspace.active].path.as_deref(),
        Some(first.join("file000.txt").as_path())
    );
    key(&mut a, "shift+f4");
    assert_eq!(
        a.workspace.tabs[a.workspace.active].path.as_deref(),
        Some(last.join("file159.txt").as_path())
    );
}

#[test]
fn streamed_find_results_follow_project_sidebar_and_f4_order() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let first = root.join("Z-root");
    let second = root.join("a-root");
    for dir in [&first, &second] {
        for path in ["a.txt", "Nested/Z.txt", "Nested/b.txt", "other/one.txt"] {
            let path = dir.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "needle\ncontext\nneedle\n").unwrap();
        }
    }
    let profile = tempfile::tempdir().unwrap();
    let mut a = app("", "Plain Text", Some(profile.path()));
    set(&mut a, "index_files", json!(false));
    add(&mut a, &first);
    add(&mut a, &second);
    begin(&mut a, "needle");
    wait_for(&mut a, |a| !a.panels.busy());
    let expected: Vec<_> = [&first, &second]
        .into_iter()
        .flat_map(|dir| {
            ["Nested/b.txt", "Nested/Z.txt", "other/one.txt", "a.txt"].map(|path| dir.join(path))
        })
        .collect();
    let headers: Vec<_> = a
        .panels
        .result_view
        .rows
        .iter()
        .filter_map(|row| match row {
            Row::Header(path) => Some(path.to_path_buf()),
            _ => None,
        })
        .collect();
    assert_eq!(headers, expected);
    let wanted: Vec<_> = expected.iter().flat_map(|p| [(p, 0), (p, 2)]).collect();
    for (key_name, steps) in [
        ("f4", wanted.clone()),
        (
            "shift+f4",
            wanted.into_iter().rev().cycle().skip(1).take(16).collect(),
        ),
    ] {
        for (path, row) in steps {
            key(&mut a, key_name);
            wait_for(&mut a, |a| {
                a.workspace.tabs[a.workspace.active].loading.is_none()
            });
            assert_eq!(
                a.workspace.tabs[a.workspace.active].path.as_ref(),
                Some(path)
            );
            let editor = a.workspace.editor();
            assert_eq!(editor.text.byte_to_line(editor.caret), row);
        }
    }
}

#[test]
fn replace_in_files_releases_recovery_and_undoes_in_open_buffer() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let open = root.join("open.txt");
    let closed = root.join("closed.txt");
    for path in [&open, &closed] {
        std::fs::write(path, "before cat\n").unwrap();
    }
    let profile = tempfile::tempdir().unwrap();
    let mut a = app("", "Plain Text", Some(profile.path()));
    set(&mut a, "index_files", json!(false));
    open_ready(&mut a, &open);
    add(&mut a, &root);
    begin(&mut a, "cat");
    wait_for(&mut a, |a| !a.panels.busy());
    named(
        &mut a,
        "show_panel",
        json!({"panel":"find_in_files","pattern":"cat","replace_pattern":"dog"}),
    );
    named(&mut a, "replace_in_files", json!({}));
    wait_for(&mut a, |a| a.panels.replacement_plan.is_some());
    a.confirm_file_replacements(|matches, files| {
        assert_eq!((matches, files), (2, 2));
        true
    });
    assert!(
        results_tab(&a)
            .editor
            .text
            .to_string()
            .ends_with("Replaced 2 matches across 2 files\n")
    );
    let index = a
        .workspace
        .tabs
        .iter()
        .position(|tab| tab.path.as_ref() == Some(&open))
        .unwrap();
    a.workspace.select(index);
    assert_eq!(a.workspace.editor().text, "before dog\n");
    assert!(!a.workspace.tabs[a.workspace.active].dirty());
    assert_eq!(std::fs::read_dir(root).unwrap().count(), 2);
    for path in [&open, &closed] {
        assert_eq!(std::fs::read_to_string(path).unwrap(), "before dog\n");
    }
    key(&mut a, "escape");
    key(&mut a, "super+z");
    assert_eq!(a.workspace.editor().text, "before cat\n");
    assert!(a.workspace.tabs[a.workspace.active].dirty());
    assert_eq!(std::fs::read_to_string(open).unwrap(), "before dog\n");
}

fn results_tab(a: &App) -> &crate::workspace::Tab {
    a.workspace
        .tabs
        .iter()
        .find(|tab| tab.find_results)
        .unwrap()
}

fn results_fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let first = root.join("a.txt");
    let second = root.join("b.txt");
    std::fs::write(&first, "before\n世界 foo\nafter\n").unwrap();
    std::fs::write(&second, "prefix foo foo\ntail\n").unwrap();
    (dir, first, second)
}

#[test]
fn r32_find_results_and_plain_text_do_not_advertise_wrap_off() {
    let (_dir, first, _) = results_fixture();
    let mut a = app("", "Plain Text", None);
    add(&mut a, first.parent().unwrap());
    key(&mut a, "super+shift+f");
    a.panels.query.set("foo");
    key(&mut a, "enter");
    wait_for(&mut a, |a| a.panels.file_complete);
    assert!(a.workspace.tabs[a.workspace.active].find_results);
    assert!(!a.workspace.editor().features.word_wrap);

    fn check(a: &mut App) {
        a.panels.message.clear();
        a.panels.layout(Size {
            width: 1200.,
            height: 600.,
        });
        let mut scene = Scene::default();
        a.panels.paint(&a.workspace, &mut scene);
        let top = 600. - STATUS * a.panels.ui_scale;
        let status: Vec<_> = scene
            .layers
            .iter()
            .flat_map(|layer| &layer.text)
            .filter(|run| f64::from(run.baseline) > top)
            .map(|run| run.text.as_str())
            .collect();
        let position = status_position(&a.workspace);
        assert!(
            status.iter().any(|label| label.contains(&position)),
            "{status:?}"
        );
        assert!(
            !status
                .iter()
                .any(|label| label.contains("wrap off") || label.contains("已停用自动换行")),
            "{status:?}"
        );
    }

    for language in ["en", "zh-Hans"] {
        a.workspace.settings.set(
            vel_config::settings::Layer::User,
            "ui_language",
            json!(language),
        );
        a.workspace.sync_ui_language();
        assert_eq!(a.workspace.translations.language().as_str(), language);
        check(&mut a);
    }
    key(&mut a, "super+n");
    named(
        &mut a,
        "set_setting",
        json!({"setting":"word_wrap","value":false}),
    );
    assert!(!a.workspace.editor().features.word_wrap);
    check(&mut a);
}

#[test]
fn find_results_tab_format_scopes_append_and_reopen() {
    let (_dir, first, second) = results_fixture();
    let mut a = app("", "Plain Text", None);
    add(&mut a, first.parent().unwrap());
    begin(&mut a, "foo");
    wait_for(&mut a, |a| a.panels.file_complete);
    let tab = results_tab(&a);
    let sheet = tab.sheet;
    assert_eq!(tab.name(), "Find Results");
    assert!(tab.path.is_none() && !tab.dirty() && !tab.editor.read_only);
    let output = tab.editor.text.to_string();
    assert_eq!(
        output,
        format!(
            "Searching 2 files for \"foo\" in <project>\n\n{}:\n      1  before\n      2: 世界 foo\n      3  after\n\n{}:\n      1: prefix foo foo\n      2  tail\n\n3 matches across 2 files\n",
            first.display(),
            second.display(),
        ),
    );
    let path = output.find(&first.to_string_lossy()[..]).unwrap();
    let row = output.find("2: 世界").unwrap();
    let hit = output.find("世界 foo").unwrap() + "世界 ".len();
    let scopes = tab.editor.exact_scopes(&[path, row]).unwrap();
    let scope = |name| vel_scope::Scope::new(name).unwrap();
    assert!(scopes[0].contains(&scope("text.find-in-files")));
    assert!(scopes[0].contains(&scope("string.unquoted.path.find-in-files")));
    assert!(scopes[1].contains(&scope("constant.numeric.line-number.find-in-files")));
    let matches = a
        .panels
        .results_tab_matches(&a.workspace.tabs[a.workspace.active], 0..output.len());
    assert_eq!(matches.len(), 3);
    assert_eq!(matches[0], hit..hit + 3);
    assert!(matches.iter().all(|r| &output[r.clone()] == "foo"));

    begin(&mut a, "prefix");
    wait_for(&mut a, |a| a.panels.file_complete);
    assert_eq!(results_tab(&a).sheet, sheet);
    let appended = results_tab(&a).editor.text.to_string();
    assert!(appended.starts_with(&output));
    assert!(appended.contains("Searching 2 files for \"prefix\""));
    assert!(appended.ends_with("1 match across 1 file\n"));
    assert_eq!(
        a.workspace.editor().caret,
        appended.find("Searching 2 files for \"prefix\"").unwrap()
    );

    input(&mut a, "editable ");
    assert!(!results_tab(&a).dirty());
    named(&mut a, "close_file", json!({}));
    assert!(a.workspace.sheet_index(sheet).is_none());
    begin(&mut a, "foo");
    wait_for(&mut a, |a| a.panels.file_complete);
    assert_ne!(results_tab(&a).sheet, sheet);
    assert_eq!(results_tab(&a).editor.text, output.as_str());
}

#[test]
fn find_results_tab_return_double_click_and_command_click_follow_edited_regions() {
    let (_dir, first, second) = results_fixture();
    let mut a = app("", "Plain Text", None);
    add(&mut a, first.parent().unwrap());
    set(&mut a, "open_files_in_new_window", json!("always"));
    begin(&mut a, "foo");
    wait_for(&mut a, |a| a.panels.file_complete);
    let sheet = results_tab(&a).sheet;
    a.workspace.editor_mut().set_caret(0, false);
    input(&mut a, "notes above the results\n");
    let output = a.workspace.editor().text.to_string();
    a.workspace
        .editor_mut()
        .set_caret(output.find("世界 foo").unwrap(), false);
    key(&mut a, "enter");
    assert_eq!(
        a.workspace.tabs[a.workspace.active].path.as_ref(),
        Some(&first)
    );
    assert_eq!(a.workspace.editor().caret, "before\n世界 ".len());

    for (target, word, count, modifiers, caret) in [
        (&second, "prefix foo foo", 2, 0, "prefix ".len()),
        (&first, "世界 foo", 1, 1 << 20, "before\n世界 ".len()),
    ] {
        a.workspace.select(a.workspace.sheet_index(sheet).unwrap());
        a.scene();
        let point = a
            .view
            .rect_at(a.workspace.editor(), output.find(word).unwrap());
        let position = Point {
            x: a.content.x + point.x + 0.5,
            y: a.content.y + point.y + a.view.line_height / 2.,
        };
        for kind in [MouseKind::Down, MouseKind::Up] {
            a.handle_mouse_event(
                None,
                MouseEvent {
                    button: 0,
                    kind,
                    position,
                    click_count: count,
                    modifiers: Modifiers(modifiers),
                    timestamp: 0.,
                },
            );
        }
        assert_eq!(
            a.workspace.tabs[a.workspace.active].path.as_ref(),
            Some(target)
        );
        assert_eq!(a.workspace.editor().caret, caret);
    }
}

#[test]
fn find_results_tab_toggle_persists_and_both_destinations_navigate() {
    let (_dir, first, second) = results_fixture();
    let mut a = app("", "Plain Text", None);
    add(&mut a, first.parent().unwrap());
    begin(&mut a, "foo");
    wait_for(&mut a, |a| a.panels.file_complete);
    let output = results_tab(&a).editor.text.clone();
    assert!(a.workspace.tabs[a.workspace.active].find_results);
    key(&mut a, "f4");
    assert_eq!(
        a.workspace.tabs[a.workspace.active].path.as_ref(),
        Some(&first)
    );
    named(&mut a, "next_result", json!({})); // Goto > Results uses this same command.
    assert_eq!(
        a.workspace.tabs[a.workspace.active].path.as_ref(),
        Some(&second)
    );
    key(&mut a, "shift+f4");
    assert_eq!(
        a.workspace.tabs[a.workspace.active].path.as_ref(),
        Some(&first)
    );

    named(
        &mut a,
        "show_panel",
        json!({"panel": "find_in_files", "pattern": "foo"}),
    );
    a.scene();
    let bounds = a.panels.tree.nodes[&37].rect;
    a.handle_mouse_event(
        None,
        MouseEvent {
            button: 0,
            kind: MouseKind::Down,
            click_count: 1,
            modifiers: Modifiers::default(),
            timestamp: 0.,
            position: Point {
                x: bounds.x + bounds.width / 2.,
                y: bounds.y + bounds.height / 2.,
            },
        },
    );
    assert!(!a.panels.find_option_checked("use_buffer"));
    named(&mut a, "toggle_find_option", json!({"option": "context"}));
    let mut state = vel_session::Window::default();
    a.panels.capture_session(&mut state);
    let state = serde_json::from_str(&serde_json::to_string(&state).unwrap()).unwrap();
    let mut panels = crate::panels::Panels::default();
    panels.restore_session(&a.workspace, &state);
    assert!(!panels.find_option_checked("use_buffer"));
    assert!(!panels.find_option_checked("context"));
    key(&mut a, "enter");
    wait_for(&mut a, |a| a.panels.file_complete);
    assert_eq!(a.panels.panel, Some(crate::panels::Panel::Results));
    assert_eq!(results_tab(&a).editor.text, output);
    assert!(
        !a.panels
            .result_view
            .rows
            .iter()
            .any(|r| matches!(r, Row::Line(l) if l.match_count == 0))
    );
    key(&mut a, "shift+f4");
    assert_eq!(
        a.workspace.tabs[a.workspace.active].path.as_ref(),
        Some(&second)
    );
    key(&mut a, "f4");
    assert_eq!(
        a.workspace.tabs[a.workspace.active].path.as_ref(),
        Some(&first)
    );

    named(
        &mut a,
        "show_panel",
        json!({"panel":"find_in_files","pattern":"foo","replace_pattern":"dog"}),
    );
    named(&mut a, "replace_in_files", json!({}));
    wait_for(&mut a, |a| a.panels.replacement_plan.is_some());
    a.confirm_file_replacements(|m, f| (m, f) == (3, 2));
    assert_eq!(a.panels.panel, Some(crate::panels::Panel::Results));
    assert_eq!(results_tab(&a).editor.text, output);
    assert_eq!(
        std::fs::read_to_string(&first).unwrap(),
        "before\n世界 dog\nafter\n"
    );
}

#[test]
fn find_results_tab_records_escape_and_panel_close_cancellation() {
    let (_dir, first, _) = results_fixture();
    let mut a = app("", "Plain Text", None);
    add(&mut a, first.parent().unwrap());
    for cancel in ["escape", "hide_panel"] {
        begin(&mut a, "foo");
        assert!(a.panels.file_task.is_some());
        if cancel == "escape" {
            key(&mut a, cancel);
        } else {
            named(&mut a, cancel, json!({}));
        }
        assert!(a.panels.file_task.is_none() && !a.panels.file_complete);
        assert!(
            results_tab(&a)
                .editor
                .text
                .to_string()
                .ends_with("Search cancelled\n")
        );
    }
    assert_eq!(
        results_tab(&a)
            .editor
            .text
            .to_string()
            .matches("Search cancelled")
            .count(),
        2
    );
}

#[test]
fn r33_cancel_find_in_files_from_menu_and_palette() {
    let (_dir, first, _) = results_fixture();
    let mut a = app("", "Plain Text", None);
    add(&mut a, first.parent().unwrap());
    let command = "cancel_find_in_files";
    let menu = a
        .application_menu()
        .into_iter()
        .find(|row| row.id == "find")
        .unwrap()
        .children
        .into_iter()
        .find(|row| row.command.as_deref() == Some(command))
        .unwrap();
    assert_eq!(menu.caption, "Cancel Find in Files");
    assert!(
        a.workspace
            .palette
            .iter()
            .any(|row| row.invocation.command == command)
    );
    assert!(!a.command_enabled(command, &menu.args));
    let idle_message = a.panels.message.clone();
    named(&mut a, command, json!({}));
    assert_eq!(a.panels.message, idle_message);

    begin(&mut a, "foo");
    assert!(a.command_enabled(command, &menu.args));
    named(&mut a, command, serde_json::from_str(&menu.args).unwrap());
    assert!(
        results_tab(&a)
            .editor
            .text
            .to_string()
            .ends_with("Search cancelled\n")
    );
    assert!(!a.command_enabled(command, &menu.args));

    named(
        &mut a,
        "show_panel",
        json!({"panel":"find_in_files","pattern":"foo","use_buffer":false}),
    );
    key(&mut a, "enter");
    assert!(a.command_enabled(command, &menu.args));
    named(
        &mut a,
        "show_overlay",
        json!({"overlay":"command_palette","text":"Find: Cancel Find in Files"}),
    );
    key(&mut a, "enter");
    assert!(a.panels.file_task.is_none() && !a.panels.file_complete);
    assert!(a.panels.message.starts_with("Search cancelled"));
    assert!(!a.command_enabled(command, &menu.args));
}

#[test]
fn r36_context_setting_counts_both_sides_and_obeys_context_toggle() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().canonicalize().unwrap().join("search.txt");
    std::fs::write(
        &path,
        "out\nleft 1\nleft 2\nneedle\nright 1\nright 2\nout\n",
    )
    .unwrap();
    let mut a = app("", "Plain Text", None);
    a.workspace.settings.set(
        vel_config::settings::Layer::User,
        "find_in_files_context_lines",
        json!(2),
    );
    for (context, expected) in [(true, vec![2, 3, 4, 5, 6]), (false, vec![4])] {
        named(
            &mut a,
            "show_panel",
            json!({"panel":"find_in_files","pattern":"needle","use_buffer":false,"context":context}),
        );
        a.panels.where_input.set(path.to_str().unwrap());
        key(&mut a, "enter");
        wait_for(&mut a, |a| a.panels.file_complete);
        let lines = a
            .panels
            .result_view
            .rows
            .iter()
            .filter_map(|row| match row {
                Row::Line(line) => Some(line.number),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(lines, expected);
    }
}

#[test]
fn r36_console_limit_evicts_oldest_and_zero_disables_history() {
    let mut a = app("", "Plain Text", None);
    for (limit, text, expected) in [
        (2, "one\ntwo\nthree", vec!["two", "three"]),
        (1, "four", vec!["four"]),
        (0, "five", vec![]),
    ] {
        a.workspace.settings.set(
            vel_config::settings::Layer::User,
            "console_max_history_lines",
            json!(limit),
        );
        a.panels.note_console(&a.workspace, text);
        assert_eq!(
            a.panels.console_history().iter().collect::<Vec<_>>(),
            expected
        );
    }
}

#[test]
fn r35_find_in_files_input_size_context_and_error_policy() {
    use vel_config::settings::Layer;
    let (_dir, first, second) = results_fixture();
    std::fs::write(&first, "世界世界世界世界 foo世界世界\n").unwrap();
    std::fs::write(&second, format!("{}foo\n", "z".repeat(160))).unwrap();
    let mut a = app("", "Plain Text", None);
    add(&mut a, first.parent().unwrap());
    a.workspace
        .settings
        .set(Layer::Window, "find_in_files_max_file_size", json!(64));
    a.workspace
        .settings
        .set(Layer::Window, "find_in_files_context_characters", json!(5));
    begin(&mut a, "foo");
    wait_for(&mut a, |a| a.panels.file_complete);
    let hits: Vec<_> = a
        .panels
        .result_view
        .rows
        .iter()
        .filter_map(|r| match r {
            Row::Line(l) if l.match_count > 0 => Some(l),
            _ => None,
        })
        .collect();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].path.as_ref(), first);
    assert_eq!(hits[0].column(), 10);
    assert!(hits[0].text.chars().count() <= 5);
    assert!(hits[0].text.contains("foo"));

    let missing = first.with_file_name("missing.txt");
    for suppress in [false, true] {
        a.workspace.settings.set(
            Layer::Window,
            "find_in_files_suppress_errors",
            json!(suppress),
        );
        named(
            &mut a,
            "show_panel",
            json!({"panel":"find_in_files", "pattern":"foo", "use_buffer":false}),
        );
        a.panels.where_input.set(missing.to_str().unwrap());
        key(&mut a, "enter");
        wait_for(&mut a, |a| a.panels.file_complete);
        assert_eq!(a.panels.message.contains("File search failed"), !suppress);
    }
}

#[test]
fn r35_find_in_files_output_budget_does_not_limit_replacement() {
    use vel_config::settings::Layer;
    let (_dir, first, second) = results_fixture();
    let mut a = app("", "Plain Text", None);
    add(&mut a, first.parent().unwrap());
    a.workspace
        .settings
        .set(Layer::Window, "find_in_files_max_result_size", json!(1));
    begin(&mut a, "foo");
    wait_for(&mut a, |a| a.panels.file_complete);
    assert!(a.panels.result_view.rows.is_empty());
    assert!(
        results_tab(&a)
            .editor
            .text
            .to_string()
            .contains("Result size limit reached")
    );
    a.panels.replace.set("bar");
    named(&mut a, "replace_in_files", json!({}));
    wait_for(&mut a, |a| a.panels.replacement_plan.is_some());
    a.confirm_file_replacements(|matches, files| {
        assert_eq!((matches, files), (3, 2));
        true
    });
    for path in [first, second] {
        let text = std::fs::read_to_string(path).unwrap();
        assert!(text.contains("bar"));
        assert!(!text.contains("foo"));
    }
}

#[test]
fn r35_current_result_preserves_next_previous_and_opens_beside_tab() {
    use vel_config::settings::Layer;
    let (_dir, first, second) = results_fixture();
    let mut a = app("", "Plain Text", None);
    add(&mut a, first.parent().unwrap());
    a.workspace
        .settings
        .set(Layer::Window, "find_in_files_side_by_side", json!(true));
    begin(&mut a, "foo");
    wait_for(&mut a, |a| a.panels.file_complete);
    let result = results_tab(&a).sheet;
    key(&mut a, "f4");
    assert_eq!(
        a.workspace.tabs[a.workspace.active].path.as_ref(),
        Some(&first)
    );
    assert_eq!(
        a.workspace.workbench.groups[a.workspace.workbench.active]
            .selected
            .len(),
        2
    );
    assert!(
        a.workspace.workbench.groups[a.workspace.workbench.active]
            .selected
            .contains(&result)
    );
    a.workspace.editor_mut().set_caret(0, false);
    named(&mut a, "current_result", json!({}));
    assert_eq!(a.workspace.editor().caret, "before\n世界 ".len());
    key(&mut a, "f4");
    assert_eq!(
        a.workspace.tabs[a.workspace.active].path.as_ref(),
        Some(&second)
    );
    key(&mut a, "shift+f4");
    assert_eq!(
        a.workspace.tabs[a.workspace.active].path.as_ref(),
        Some(&first)
    );
}

#[test]
fn r35_syntax_tests_menu_palette_output_legacy_and_result_links() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir
        .path()
        .canonicalize()
        .unwrap()
        .join("syntax_test_r35.rs");
    let syntax = "Packages/Rust/Rust.sublime-syntax";
    std::fs::write(
        &file,
        format!("// SYNTAX TEST \"{syntax}\"\nfn test() {{}}\n// <- no.such.scope\n"),
    )
    .unwrap();
    let mut a = app("", "Plain Text", None);
    a.workspace
        .open(crate::cli::FileLocation {
            path: file.clone(),
            line: 1,
            column: 1,
        })
        .unwrap();
    let menu = a
        .application_menu()
        .into_iter()
        .find(|row| row.caption == "Tools")
        .unwrap()
        .children
        .into_iter()
        .find(|row| row.command.as_deref() == Some("run_syntax_tests"))
        .unwrap();
    named(
        &mut a,
        menu.command.as_deref().unwrap(),
        serde_json::from_str(&menu.args).unwrap(),
    );
    let finish = |a: &mut App| {
        let deadline = Instant::now() + crate::test_timing::HANG_GUARD;
        while !a.builds.poll(&mut a.panels) {
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
    };
    finish(&mut a);
    assert_eq!(a.panels.panel, Some(crate::panels::Panel::Output));
    assert!(
        a.panels.output.contains("0 passed, 1 failed"),
        "{}",
        a.panels.output
    );
    assert_eq!(a.panels.build_results[0].1.path, file);
    key(&mut a, "f4");
    assert_eq!(
        a.workspace
            .editor()
            .text
            .byte_to_line(a.workspace.editor().caret),
        2
    );

    let mut legacy = b"// SYNTAX TEST \"not loaded\"\n// caf".to_vec();
    legacy.extend_from_slice(b"\xe9\n// <- source.rust\n");
    std::fs::write(&file, legacy).unwrap();
    let invocation = a
        .workspace
        .palette
        .iter()
        .find(|row| row.invocation.command == "run_syntax_tests")
        .unwrap()
        .invocation
        .clone();
    assert_eq!(invocation.command, "run_syntax_tests");
    named(
        &mut a,
        &invocation.command,
        json!({"file":file, "syntax":"Rust"}),
    );
    finish(&mut a);
    assert!(
        a.panels.output.contains("Non-UTF-8 syntax test decoded as"),
        "{}",
        a.panels.output
    );
    assert!(
        a.panels.output.contains("1 passed, 0 failed"),
        "{}",
        a.panels.output
    );
    assert_eq!(a.builds.diagnostics[0].line, 2);
}

#[test]
fn r35_io_workers_and_automatic_syntax_size_setting_reach_file_open() {
    use vel_config::settings::Layer;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().canonicalize().unwrap().join("fixture.rs");
    let source = "fn fixture() {}\n";
    std::fs::write(&path, source).unwrap();
    let mut a = app("", "Plain Text", None);
    a.workspace
        .settings
        .set(Layer::Window, "io_workers", json!(1));
    a.workspace.settings.set(
        Layer::Window,
        "syntax_detection_size_limit",
        json!(source.len() - 1),
    );
    a.workspace
        .open(crate::cli::FileLocation {
            path: path.clone(),
            line: 1,
            column: 1,
        })
        .unwrap();
    assert!(a.workspace.tabs[a.workspace.active].loading.is_some());
    wait_for(&mut a, |a| {
        a.workspace.tabs[a.workspace.active].loading.is_none()
    });
    let syntax = |a: &App| {
        a.workspace
            .editor()
            .language
            .borrow()
            .as_ref()
            .unwrap()
            .name
            .clone()
    };
    assert_eq!(syntax(&a), "Plain Text");
    a.workspace.set_file_type("Rust").unwrap();
    assert_eq!(syntax(&a), "Rust");
    let second = path.with_file_name("exact.rs");
    std::fs::write(&second, source).unwrap();
    a.workspace.settings.set(
        Layer::Window,
        "syntax_detection_size_limit",
        json!(source.len()),
    );
    a.workspace
        .open(crate::cli::FileLocation {
            path: second,
            line: 1,
            column: 1,
        })
        .unwrap();
    wait_for(&mut a, |a| {
        a.workspace.tabs[a.workspace.active].loading.is_none()
    });
    assert_eq!(syntax(&a), "Rust");
    a.workspace.save(a.workspace.active, None).unwrap();
}

#[test]
fn find_results_tab_streams_while_typing_within_frame_budget() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    for i in 0..4 {
        std::fs::write(
            root.join(format!("hits-{i}.txt")),
            "prefix needle 人\n".repeat(8_000),
        )
        .unwrap();
    }
    let mut a = app("draft\n", "Plain Text", None);
    let draft = a.workspace.tabs[a.workspace.active].sheet;
    add(&mut a, &root);
    begin(&mut a, "needle");
    a.workspace.select(a.workspace.sheet_index(draft).unwrap());
    let deadline = Instant::now() + crate::test_timing::HANG_GUARD;
    let mut samples = Vec::new();
    let mut streamed = false;
    while !a.panels.file_complete {
        let start = Instant::now();
        let viewport = a.viewport();
        a.panels.poll(&mut a.workspace, viewport);
        input(&mut a, "x");
        samples.push(start.elapsed());
        streamed |= !a.panels.file_complete && results_tab(&a).editor.text.len_lines() > 100;
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(1));
    }
    samples.sort_unstable();
    let p95 = samples[samples.len() * 95 / 100];
    // The behaviour: results stream into the tab while every key still lands (below).
    // The 16 ms frame budget is a performance contract, checked in timing runs only.
    assert!(streamed && samples.len() > 10);
    crate::test_timing::within(p95, Duration::from_millis(16), "typing + streaming p95");
    assert!(
        a.workspace
            .editor()
            .text
            .to_string()
            .ends_with(&"x".repeat(samples.len()))
    );
    assert!(
        results_tab(&a)
            .editor
            .text
            .to_string()
            .ends_with("32000 matches across 4 files\n")
    );
}

#[test]
fn replace_in_files_reports_a_changed_target_without_overwriting_it() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let file = root.join("changed.txt");
    std::fs::write(&file, "cat\n").unwrap();
    let profile = tempfile::tempdir().unwrap();
    let mut a = app("", "Plain Text", Some(profile.path()));
    set(&mut a, "index_files", json!(false));
    add(&mut a, &root);
    begin(&mut a, "cat");
    wait_for(&mut a, |a| !a.panels.busy());
    named(
        &mut a,
        "show_panel",
        json!({"panel":"find_in_files","pattern":"cat","replace_pattern":"dog"}),
    );
    named(&mut a, "replace_in_files", json!({}));
    wait_for(&mut a, |a| a.panels.replacement_plan.is_some());
    a.confirm_file_replacements(|_, _| {
        std::fs::write(&file, "bat\n").unwrap();
        true
    });
    assert_eq!(std::fs::read_to_string(file).unwrap(), "bat\n");
    assert_eq!(std::fs::read_dir(root).unwrap().count(), 1);
    assert!(
        a.panels
            .message
            .contains("changed after replacement preview")
    );
}

#[cfg(unix)]
#[test]
fn replace_in_files_removes_recovery_after_the_writer_refuses_read_only_target() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let file = root.join("read-only.txt");
    std::fs::write(&file, "cat\n").unwrap();
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o444)).unwrap();
    let mut a = app("", "Plain Text", None);
    add(&mut a, &root);
    begin(&mut a, "cat");
    wait_for(&mut a, |a| !a.panels.busy());
    named(
        &mut a,
        "show_panel",
        json!({"panel":"find_in_files","pattern":"cat","replace_pattern":"dog"}),
    );
    named(&mut a, "replace_in_files", json!({}));
    wait_for(&mut a, |a| a.panels.replacement_plan.is_some());
    a.confirm_file_replacements(|_, _| true);
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "cat\n");
    assert!(
        a.panels.message.contains("read-only") || a.panels.message.contains("Permission denied"),
        "{}",
        a.panels.message
    );
    assert_eq!(std::fs::read_dir(root).unwrap().count(), 1);
}
