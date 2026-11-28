use super::*;
use crate::cli::FileLocation;
use crate::panels::Overlay;
use std::fs;
use std::path::Path;
use vel_config::keymap::Key;
use vel_platform::Modifiers;
use vel_text::Text;

fn workspace_app(root: Option<PathBuf>, text: &str) -> App {
    let now = Instant::now();
    let options = Options::default();
    App::new(
        options.clone(),
        Clock::new(now, std::time::SystemTime::now()),
        Reporter::for_options(&options, now).unwrap(),
        Workspace::new(Text::from(text), root.clone(), None, false).unwrap(),
        Arc::new(AtomicBool::new(false)),
        std::rc::Rc::new(std::cell::RefCell::new(crate::session::Sessions::new(
            root.as_deref(),
        ))),
    )
}

fn location(path: &Path) -> FileLocation {
    FileLocation {
        path: path.into(),
        line: 1,
        column: 1,
    }
}

fn key(a: &mut App, chord: &str) {
    assert!(
        a.dispatch_key_binding(None, &Key::parse(chord).unwrap(), Instant::now(), 0..0),
        "{chord}"
    );
}

#[track_caller]
fn until(a: &mut App, ready: impl Fn(&mut App) -> bool) {
    let deadline = Instant::now() + crate::test_timing::HANG_GUARD;
    loop {
        a.panels.poll(&mut a.workspace, 0..0);
        a.sync_active_view();
        if ready(a) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "picker did not publish: {}",
            a.panels.message
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn goto(a: &mut App, text: &str) {
    a.run_named(None, "show_overlay", &json!({"overlay":"goto","text":text}));
    assert_eq!(a.panels.overlay, Some(Overlay::Goto));
    until(a, |a| {
        !a.panels.ranking_pending() && !a.panels.quick.items.is_empty()
    });
}

#[test]
fn r32_goto_right_opens_and_retains_picker_but_escape_restores_origin() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let first = root.join("alpha.txt");
    let second = root.join("beta.txt");
    fs::write(&first, "alpha").unwrap();
    fs::write(&second, "beta").unwrap();
    let mut a = workspace_app(None, "original");
    a.panels.add_folder(&mut a.workspace, root).unwrap();
    let origin = a.workspace.tabs[a.workspace.active].sheet;
    a.workspace.editor_mut().set_caret(3, false);
    goto(&mut a, "txt");
    assert_eq!(a.panels.quick.items.len(), 2);
    let preview = a.workspace.tabs[a.workspace.active].sheet;
    assert!(a.workspace.tabs[a.workspace.active].goto_preview);

    // Cocoa uses ordinary text selectors when the key itself is not consumed.
    for (chord, selector) in [
        ("left", Command::Left(false)),
        ("right", Command::Right(false)),
    ] {
        if !a.dispatch_key_binding(None, &Key::parse(chord).unwrap(), Instant::now(), 0..0) {
            assert!(
                a.panels
                    .command(&mut a.workspace, selector, 0..0)
                    .unwrap()
                    .0
            );
        }
    }
    assert!(a.panels.input().unwrap().caret_at_end());
    assert!(
        a.workspace.tabs[a.workspace.active].goto_preview,
        "caret motion did not open the item"
    );
    key(&mut a, "right");
    assert_eq!(a.panels.overlay, Some(Overlay::Goto));
    let tab = &a.workspace.tabs[a.workspace.sheet_index(preview).unwrap()];
    assert!(!tab.goto_preview && !tab.transient && !tab.preview_recent_pending);
    assert!(
        a.workspace
            .recent_files
            .contains(tab.path.as_ref().unwrap())
    );

    key(&mut a, "down");
    assert!(a.workspace.tabs[a.workspace.active].goto_preview);
    key(&mut a, "escape");
    assert!(a.panels.overlay.is_none());
    assert_eq!(a.workspace.tabs[a.workspace.active].sheet, origin);
    assert_eq!(a.workspace.editor().caret, 3);
    assert_eq!(a.workspace.tabs.len(), 2);
    assert!(a.workspace.sheet_index(preview).is_some());
}

#[test]
fn r32_goto_word_highlights_cover_local_and_qualified_fuzzy_matches() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let path = root.join("target.txt");
    let text = format!("far_baz\nfb foo_bar cafe\n{}", "fb\n".repeat(106));
    fs::write(&path, &text).unwrap();
    let mut a = workspace_app(None, &text);
    a.panels.add_folder(&mut a.workspace, root).unwrap();

    for (query, exit) in [("#fb", "escape"), ("target#fb", "enter")] {
        goto(&mut a, query);
        let ranges = a.panels.current_highlights(&a.workspace);
        assert_eq!(
            &ranges[..5],
            &[0..1, 4..5, 8..10, 11..12, 15..16],
            "{query}"
        );
        assert_eq!(ranges.len(), 111, "not capped to the picker row count");
        assert_eq!(
            a.workspace.editor().caret,
            if query == "#fb" { 8 } else { 0 }
        );
        key(&mut a, exit);
        assert!(
            a.panels.current_highlights(&a.workspace).is_empty(),
            "{exit}"
        );
    }
    goto(&mut a, "#FB");
    assert_eq!(a.panels.current_highlights(&a.workspace).len(), 111);
    key(&mut a, "right");
    assert_eq!(a.panels.overlay, Some(Overlay::Goto));
    assert!(a.panels.current_highlights(&a.workspace).is_empty());
    key(&mut a, "escape");
    assert_eq!(
        a.workspace.tabs[a.workspace.active].path.as_ref(),
        Some(&path)
    );
}

fn click(a: &mut App, position: Point, modifiers: u64) {
    for kind in [MouseKind::Down, MouseKind::Up] {
        a.handle_mouse_event(
            None,
            MouseEvent {
                kind,
                position,
                modifiers: Modifiers(modifiers),
                button: 0,
                click_count: 1,
                timestamp: 0.,
            },
        );
    }
}

fn center(rect: Rect) -> Point {
    Point {
        x: rect.x + rect.width / 2.,
        y: rect.y + rect.height / 2.,
    }
}

fn tab_point(a: &mut App, sheet: u64) -> Point {
    a.scene();
    center(a.tab_hits.iter().find(|(id, _)| *id == sheet).unwrap().1)
}

#[test]
fn r32_option_click_replaces_focused_tab_from_strip_dropdown_and_sidebar() {
    const CMD: u64 = 1 << 20;
    const OPT: u64 = 1 << 19;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let paths = ["alpha.txt", "beta.txt", "gamma.txt", "delta.txt"].map(|p| root.join(p));
    for path in &paths {
        fs::write(path, path.file_stem().unwrap().to_str().unwrap()).unwrap();
    }

    for surface in ["strip", "dropdown", "sidebar open files", "sidebar folder"] {
        let mut a = workspace_app(None, "");
        a.panels.add_folder(&mut a.workspace, root.clone()).unwrap();
        let mut sheets = vec![];
        for path in &paths[..3] {
            a.workspace.open(location(path)).unwrap();
            sheets.push(a.workspace.tabs[a.workspace.active].sheet);
        }
        let p = tab_point(&mut a, sheets[0]);
        click(&mut a, p, 0);
        let p = tab_point(&mut a, sheets[1]);
        click(&mut a, p, CMD);
        let layout = a.workspace.workbench.layout.clone();
        assert_eq!(
            a.workspace.workbench.groups[0].visible(),
            sheets[..2],
            "{surface}"
        );
        let active = match surface {
            "strip" => {
                let p = tab_point(&mut a, sheets[2]);
                click(&mut a, p, OPT);
                sheets[2]
            }
            "dropdown" => {
                let (_, _, rect) = a
                    .tab_button_hits
                    .iter()
                    .find(|(_, class, _)| *class == "tab_dropdown_button")
                    .copied()
                    .unwrap();
                click(&mut a, center(rect), 0);
                until(&mut a, |a| !a.panels.ranking_pending());
                a.scene();
                let row = a
                    .panels
                    .quick
                    .items
                    .iter()
                    .position(|i| i.caption == "gamma.txt")
                    .unwrap();
                let p = Point {
                    x: a.panels.quick_bounds.x + 12.,
                    y: a.panels.quick_list_top()
                        + (row as f64 + 0.5) * a.panels.quick.scroll.row_height
                        - a.panels.quick.scroll.offset,
                };
                click(&mut a, p, OPT);
                assert!(a.panels.overlay.is_none());
                sheets[2]
            }
            "sidebar open files" => {
                a.panels.open_files_visible = true;
                a.scene();
                let p = Point {
                    x: a.panels.side_bounds.x + 36.,
                    y: a.panels.side_bounds.y + 3.5 * 24. * a.panels.ui_scale,
                };
                click(&mut a, p, OPT);
                sheets[2]
            }
            "sidebar folder" => {
                until(&mut a, |a| {
                    a.scene();
                    a.panels.sidebar_has_row(&paths[3]) && !a.panels.sidebar_animates()
                });
                let p = a.panels.test_sidebar_point(&a.workspace, &paths[3]);
                click(&mut a, p, OPT);
                assert_eq!(
                    a.workspace.tabs[a.workspace.active].path.as_ref(),
                    Some(&paths[3])
                );
                assert!(!a.workspace.tabs[a.workspace.active].transient);
                a.workspace.tabs[a.workspace.active].sheet
            }
            _ => unreachable!(),
        };
        let group = &a.workspace.workbench.groups[0];
        assert_eq!(group.visible(), [sheets[0], active], "{surface}");
        assert_eq!(group.focused, Some(active));
        assert_eq!(group.anchor, Some(active));
        assert_eq!(a.workspace.workbench.layout, layout);
        assert!(
            a.workspace.sheet_index(sheets[1]).is_some(),
            "replaced member is still open"
        );
    }
}

#[test]
fn r32_recoverable_error_is_localized_transient_status() {
    let mut a = workspace_app(None, "untouched");
    a.run_named(None, "set_ui_language", &json!({"language":"zh-Hans"}));
    a.run_named(None, "save_encoding", &json!({}));
    assert_eq!(a.panels.message, "未指定编码");
    assert!(a.panels.message_deadline().is_some());
    assert!(a.data_loss_dialogs.is_empty());
    assert_eq!(a.workspace.editor().text, "untouched");
    a.named_error(None, "a missing fixture resource");
    assert_eq!(a.panels.message, "操作失败：a missing fixture resource");
}

#[test]
fn r32_save_and_hot_exit_failures_use_dialogs_and_keep_unsaved_text() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let mut a = workspace_app(Some(root.into()), "");
    a.run_named(None, "set_ui_language", &json!({"language":"zh-Hans"}));
    a.run_named(None, "insert", &json!({"characters":"must not be lost"}));
    let blocker = root.join("blocker");
    fs::write(&blocker, "not a directory").unwrap();
    a.run_named(
        None,
        "save_and_close",
        &json!({"file":blocker.join("cannot-save.txt")}),
    );
    assert_eq!(a.data_loss_dialogs.len(), 1);
    assert_eq!(a.data_loss_dialogs[0].title, "Velocity 错误");
    assert!(a.data_loss_dialogs[0].detail.starts_with("无法保存："));
    assert_eq!(a.workspace.editor().text, "must not be lost");
    assert!(a.workspace.tabs[a.workspace.active].dirty());
    assert!(!a.empty_close_requested);

    fs::write(root.join("Sessions"), "block session writes").unwrap();
    assert!(!a.prepare_quit(None));
    assert_eq!(a.data_loss_dialogs.len(), 2);
    assert!(
        a.data_loss_dialogs[1]
            .detail
            .starts_with("无法保存热退出会话：")
    );
    assert_eq!(a.workspace.editor().text, "must not be lost");
}
