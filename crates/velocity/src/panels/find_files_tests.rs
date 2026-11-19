//! Find in Files through real workers, loaders and panel key bindings.
use super::*;
use vel_search::files::Row;

fn wait_for(a: &mut App, ready: impl Fn(&App) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
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
        let deadline = Instant::now() + Duration::from_secs(10);
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
