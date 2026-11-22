use super::*;
use crate::cli::FileLocation;
use crate::session::Sessions;
use std::{cell::RefCell, fs, rc::Rc};
use vel_config::keymap::Key;
use vel_platform::accessibility::Role;
use vel_text::Text;

fn app(workspace: Workspace, sessions: Rc<RefCell<Sessions>>) -> App {
    let options = Options::default();
    let now = Instant::now();
    App::new(
        options.clone(),
        Clock::new(now, std::time::SystemTime::now()),
        Reporter::for_options(&options, now).unwrap(),
        workspace,
        Arc::new(AtomicBool::new(false)),
        sessions,
    )
}

fn fixture(name: &str) -> (tempfile::TempDir, PathBuf, App) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().canonicalize().unwrap().join(name);
    fs::write(&path, "original\n").unwrap();
    let mut workspace = Workspace::new(Text::new(), None, None, false).unwrap();
    workspace
        .open(FileLocation::path_only(path.clone()))
        .unwrap();
    (
        dir,
        path,
        app(workspace, Rc::new(RefCell::new(Sessions::new(None)))),
    )
}

fn center(rect: Rect) -> Point {
    Point {
        x: rect.x + rect.width / 2.,
        y: rect.y + rect.height / 2.,
    }
}

fn mouse(app: &mut App, point: Point, click_count: usize, kind: MouseKind) {
    app.handle_mouse_event(
        None,
        MouseEvent {
            button: 0,
            kind,
            position: point,
            click_count,
            modifiers: Modifiers::default(),
            timestamp: 0.,
        },
    );
}

fn click(app: &mut App, point: Point, count: usize) {
    mouse(app, point, count, MouseKind::Down);
    mouse(app, point, count, MouseKind::Up);
}

fn tab(app: &mut App, index: usize) -> Rect {
    app.scene();
    let id = app.workspace.tabs[index].sheet;
    app.tab_hits
        .iter()
        .find(|(sheet, _)| *sheet == id)
        .unwrap()
        .1
}

fn double_click_tab(app: &mut App, index: usize) -> Rect {
    let rect = tab(app, index);
    let point = center(rect);
    click(app, point, 1);
    app.scene();
    click(app, point, 2);
    rect
}

fn start(app: &mut App) -> Rect {
    double_click_tab(app, app.workspace.active)
}

fn key(app: &mut App, chord: &str) -> bool {
    app.dispatch_key_binding(None, &Key::parse(chord).unwrap(), Instant::now(), 0..0)
}

fn insert(app: &mut App, text: &str) {
    app.handle_text_input(
        None,
        TextInput::Insert {
            text: text.into(),
            replacement: None,
        },
    );
}

fn value_and_selection(app: &App) -> (String, String) {
    let state = app.text_input_state();
    let range = state.selected.bytes(&state.text);
    let value = state.text.to_string();
    let selected = value[range].to_owned();
    (value, selected)
}

#[test]
fn permanent_tab_selects_the_stem_and_suppresses_tab_controls() {
    for (name, stem) in [
        ("新建文本文档.txt", "新建文本文档"),
        ("archive.tar.gz", "archive.tar"),
        ("LICENSE", "LICENSE"),
        (".zshrc", ".zshrc"),
    ] {
        let (_dir, _, mut app) = fixture(name);
        let sheet = app.workspace.tabs[app.workspace.active].sheet;
        app.run_named(None, "insert", &json!({"characters":"dirty"}));
        let before = start(&mut app);
        let scene = app.scene();
        let after = app.tab_rename.as_ref().unwrap().bounds;
        assert_eq!(after, before);
        assert!(!app.view.focused);
        assert_eq!(value_and_selection(&app), (name.into(), stem.into()));
        assert!(!app.tab_close_hits.iter().any(|(id, _)| *id == sheet));
        assert!(
            !scene
                .layers
                .iter()
                .flat_map(|l| &l.text)
                .any(|r| r.text == "•")
        );
    }
}

#[test]
fn preview_is_only_promoted_and_empty_bar_still_opens_new_file() {
    let (_dir, _, mut app) = fixture("origin.txt");
    app.workspace.tabs[0].transient = true;
    let point = center(start(&mut app));
    assert!(!app.workspace.tabs[0].transient);
    assert!(app.tab_rename.is_none());
    click(&mut app, point, 3);
    assert!(
        app.tab_rename.is_none(),
        "the same click sequence must not rename"
    );
    app.scene();
    let (_, bar, _) = app.tab_bars[0];
    let empty = Point {
        x: bar.x + bar.width - 10.,
        y: point.y,
    };
    assert!(
        !app.tab_hits
            .iter()
            .any(|(_, r)| vel_ui::contains(*r, empty))
    );
    click(&mut app, empty, 1);
    click(&mut app, empty, 2);
    assert_eq!(app.workspace.tabs.len(), 2);
    assert!(app.workspace.tabs[app.workspace.active].path.is_none());
    assert!(app.tab_rename.is_none());
}

#[test]
fn virtual_settings_find_results_read_only_and_byte_views_are_ineligible() {
    for kind in [
        "untitled", "settings", "results", "resource", "binary", "remote", "missing",
    ] {
        let (_dir, path, mut app) = fixture("real.txt");
        let tab = &mut app.workspace.tabs[0];
        match kind {
            "untitled" => tab.path = None,
            "settings" => {
                tab.editor
                    .settings
                    .set(Layer::View, "velocity_settings_label", json!("Settings"))
            }
            "results" => tab.find_results = true,
            "resource" => tab.editor.read_only = true,
            "binary" => tab.binary_document = true,
            "remote" => tab.path = Some(PathBuf::from("sftp://example.invalid/real.txt")),
            "missing" => fs::remove_file(path).unwrap(),
            _ => unreachable!(),
        }
        let rect = start(&mut app);
        assert!(app.tab_rename.is_none(), "{kind}");
        assert!(
            app.headless_context_menu(center(rect))
                .iter()
                .all(|item| { item.command.as_deref() != Some(RENAME_TAB) }),
            "{kind}"
        );
    }
}

#[test]
fn return_retargets_clones_live_windows_lazy_tabs_and_session_without_saving_edits() {
    let (_dir, from, mut app) = fixture("notes.txt");
    let sibling = from.with_file_name("sibling.txt");
    fs::write(&sibling, "sibling").unwrap();
    let mut linked = Workspace::new(Text::new(), None, None, false).unwrap();
    linked.open(FileLocation::path_only(from.clone())).unwrap();
    linked.open(FileLocation::path_only(sibling)).unwrap();
    let (window, docs) = linked.snapshot().unwrap();
    let mut lazy = Workspace::new(Text::new(), None, None, false).unwrap();
    lazy.restore_window(&window, &docs).unwrap();
    let mut lazy = self::app(lazy, app.sessions.clone());
    assert!(lazy.workspace.tabs[0].lazy.is_some());
    lazy.checkpoint(false).unwrap();
    let mut second = self::app(linked, app.sessions.clone());
    second.workspace.select(0);
    second.run_named(None, "insert", &json!({"characters":"other window "}));
    second.checkpoint(false).unwrap();

    app.run_named(None, "insert", &json!({"characters":"unsaved "}));
    app.run_named(None, "clone_file", &json!({}));
    app.checkpoint(false).unwrap();
    let document = app.workspace.tabs[app.workspace.active].document;
    start(&mut app);
    key(&mut app, "super+a");
    insert(&mut app, "renamed.rs");
    assert!(key(&mut app, "enter"));

    let to = from.with_file_name("renamed.rs");
    assert!(!from.exists());
    assert_eq!(fs::read_to_string(&to).unwrap(), "original\n");
    assert!(app.tab_rename.is_none());
    assert_eq!(app.workspace.tabs.len(), 2);
    assert!(
        app.workspace.tabs.iter().all(|tab| {
            tab.path.as_ref() == Some(&to) && tab.document == document && tab.dirty()
        })
    );
    assert!(
        app.workspace
            .editor()
            .language
            .borrow()
            .as_ref()
            .unwrap()
            .syntax
            .contains("Rust")
    );
    assert!(app.workspace.recent_files.contains(&to));
    assert!(!app.workspace.recent_files.contains(&from));
    app.checkpoint(false).unwrap();
    let snapshot = app.sessions.borrow().test_snapshot();
    assert!(
        snapshot
            .documents
            .iter()
            .all(|doc| doc.path.as_ref() != Some(&from))
    );
    assert!(
        snapshot
            .documents
            .iter()
            .filter(|doc| doc.path.as_ref() == Some(&to))
            .count()
            >= 2
    );
    assert!(snapshot.recent_files.contains(&to));
    assert!(!snapshot.recent_files.contains(&from));

    assert!(second.poll_workspace_model());
    assert!(lazy.poll_workspace_model());
    assert_eq!(second.workspace.tabs[0].path.as_ref(), Some(&to));
    assert!(second.workspace.tabs[0].dirty());
    assert_eq!(
        second.workspace.editor().text.to_string(),
        "other window original\n"
    );
    let (pending, _) = lazy.workspace.tabs[0]
        .lazy
        .as_ref()
        .expect("rename must not load the hidden tab");
    assert_eq!(pending.path.as_ref(), Some(&to));
    lazy.workspace.select(0);
    assert_eq!(lazy.workspace.editor().text.to_string(), "original\n");
    for candidate in [&mut app, &mut second] {
        candidate.run_named(None, "undo", &json!({}));
        assert_eq!(candidate.workspace.editor().text.to_string(), "original\n");
        assert!(!candidate.workspace.tabs[candidate.workspace.active].dirty());
    }
}

#[test]
fn escape_cancels_and_unchanged_return_does_not_touch_the_file() {
    let (_dir, from, mut app) = fixture("origin.txt");
    let stamp = fs::metadata(&from).unwrap().modified().unwrap();
    start(&mut app);
    assert!(key(&mut app, "enter"));
    assert!(app.tab_rename.is_none());
    start(&mut app);
    insert(&mut app, "cancelled");
    assert!(key(&mut app, "escape"));
    assert!(app.tab_rename.is_none());
    assert!(!from.with_file_name("cancelled.txt").exists());
    assert_eq!(fs::metadata(&from).unwrap().modified().unwrap(), stamp);
    assert_eq!(app.text_input_state().text.to_string(), "original\n");
    app.scene();
    assert!(app.view.focused);
}

#[test]
fn click_away_tab_switch_and_key_window_loss_commit() {
    for trigger in ["editor", "tab", "command", "wheel", "window"] {
        let (_dir, from, mut app) = fixture("origin.txt");
        let other = from.with_file_name("sibling.txt");
        fs::write(&other, "sibling").unwrap();
        app.workspace.open(FileLocation::path_only(other)).unwrap();
        app.workspace.select(0);
        start(&mut app);
        insert(&mut app, "renamed");
        match trigger {
            "editor" => {
                let p = center(app.content);
                click(&mut app, p, 1);
            }
            "tab" => {
                let p = center(tab(&mut app, 1));
                click(&mut app, p, 1);
                assert_eq!(app.workspace.active, 1);
            }
            "command" => app.run_named(None, "next_view", &json!({})),
            "wheel" => {
                app.workspace
                    .settings
                    .set(Layer::User, "mouse_wheel_switches_tabs", json!(true));
                let position = center(app.tab_rename.as_ref().unwrap().bounds);
                app.handle_scroll_event(
                    None,
                    ScrollEvent {
                        position,
                        delta_x: 0.,
                        delta_y: -1.,
                        precise: false,
                        phase: 0,
                        momentum_phase: 0,
                        modifiers: Modifiers::default(),
                        timestamp: 0.,
                    },
                );
                assert_eq!(app.workspace.active, 1);
            }
            "window" => {
                // Same commit path as the platform's InlineTextFocusLost event.
                assert!(app.inline_text_input());
                assert!(app.finish_tab_rename(None));
            }
            _ => unreachable!(),
        }
        assert!(app.tab_rename.is_none(), "{trigger}");
        assert!(from.with_file_name("renamed.txt").is_file(), "{trigger}");
    }
}

#[test]
fn errors_never_overwrite_and_keep_the_field_focused_with_full_selection() {
    let (_dir, from, mut app) = fixture("origin.txt");
    let occupied = from.with_file_name("taken.txt");
    fs::write(&occupied, "keep me").unwrap();
    start(&mut app);
    insert(&mut app, "taken");
    // A failed click-away must not allow editing commands into the document.
    let point = center(app.content);
    click(&mut app, point, 1);
    assert!(
        app.panels.message.contains("already exists"),
        "{}",
        app.panels.message
    );
    assert_eq!(
        value_and_selection(&app),
        ("taken.txt".into(), "taken.txt".into())
    );
    assert!(app.accessible_tree().nodes.iter().any(|node| {
        node.role == Role::Status
            && node
                .announcement
                .as_deref()
                .is_some_and(|status| status.contains("already exists"))
    }));
    assert_eq!(fs::read_to_string(&occupied).unwrap(), "keep me");
    for bad in ["", ".", "..", "nested/name", "nested\\name", "nul\0"] {
        insert(&mut app, bad);
        assert!(key(&mut app, "enter"));
        assert!(
            app.panels.message.contains("without a path"),
            "{}",
            app.panels.message
        );
        assert_eq!(value_and_selection(&app), (bad.into(), bad.into()));
        assert!(from.is_file());
    }
    key(&mut app, "escape");
    assert!(app.tab_rename.is_none());
}

#[test]
fn case_only_rename_updates_the_actual_directory_entry() {
    let (_dir, from, mut app) = fixture("case.txt");
    start(&mut app);
    insert(&mut app, "CASE");
    key(&mut app, "enter");
    assert!(app.tab_rename.is_none());
    assert_eq!(
        app.workspace.tabs[0].path.as_ref(),
        Some(&from.with_file_name("CASE.txt"))
    );
    let names: Vec<_> = fs::read_dir(from.parent().unwrap())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(names, ["CASE.txt"]);
}

#[test]
fn pinyin_preedit_uses_the_tab_geometry_then_commits_before_rename() {
    let (_dir, from, mut app) = fixture("新建文本文档.txt");
    start(&mut app);
    app.handle_text_input(
        None,
        TextInput::Marked {
            text: "ni hao".into(),
            selected: Utf16Range::new(6, 0),
            replacement: None,
        },
    );
    let scene = app.scene();
    let state = app.text_input_state();
    assert_eq!(state.text.to_string(), "ni hao.txt");
    let marked = state.marked.unwrap();
    assert_eq!(marked, Utf16Range::new(0, 6));
    let (rect, actual) = app.text_input_rect(marked);
    assert_eq!(marked, actual);
    let bounds = app.tab_rename.as_ref().unwrap().bounds;
    assert_eq!(rect.y + rect.height, bounds.y + bounds.height);
    let underline = scene
        .layers
        .iter()
        .flat_map(|l| &l.underlines)
        .find(|u| u.thickness == 1. && vel_ui::contains(bounds, center(u.rect)))
        .unwrap();
    assert_eq!(rect.x, underline.rect.x);
    assert!(from.exists());
    assert!(
        !key(&mut app, "enter"),
        "the input method owns Return during preedit"
    );
    insert(&mut app, "你好");
    assert!(app.text_input_state().marked.is_none());
    assert_eq!(app.text_input_state().text.to_string(), "你好.txt");
    assert!(from.exists());
    key(&mut app, "enter");
    assert!(from.with_file_name("你好.txt").exists());
}

#[test]
fn field_edits_scroll_and_undo_without_editor_bindings_or_tab_dragging() {
    let (_dir, _, mut app) = fixture("origin.txt");
    let sheet = app.workspace.tabs[0].sheet;
    start(&mut app);
    app.scene();
    let field = app.tab_rename.as_ref().unwrap().bounds;
    let editor = center(app.content);
    mouse(&mut app, center(field), 1, MouseKind::Down);
    mouse(&mut app, editor, 1, MouseKind::Drag);
    mouse(&mut app, editor, 1, MouseKind::Up);
    assert!(app.panels.dragging_tab.is_none());
    assert_eq!(app.workspace.tabs[app.workspace.active].sheet, sheet);
    assert!(key(&mut app, "super+w"));
    assert!(app.tab_rename.is_some());
    assert!(
        !key(&mut app, "a"),
        "printable text must go through native text input"
    );
    assert!(key(&mut app, "super+a"));
    assert!(key(&mut app, "super+c"));
    assert_eq!(
        app.tab_rename.as_ref().unwrap().input.clipboard(),
        "origin.txt"
    );
    let long = format!("{}.txt", "w".repeat(180));
    app.tab_rename
        .as_mut()
        .unwrap()
        .input
        .set_clipboard(long.clone());
    assert!(key(&mut app, "super+v"));
    app.scene();
    assert_eq!(app.text_input_state().text.to_string(), long);
    assert!(app.tab_rename.as_ref().unwrap().input.scroll > 0);
    app.tab_rename_command(None, Command::Left(true));
    assert_eq!(value_and_selection(&app).1, "t");
    assert!(key(&mut app, "super+z"));
    assert_eq!(app.text_input_state().text.to_string(), "origin.txt");
    assert_eq!(app.workspace.editor().text.to_string(), "original\n");
    assert!(!app.workspace.tabs[0].dirty());
}

#[test]
fn context_menu_opens_the_clicked_tabs_localized_accessible_field() {
    for (locale, menu, label) in [
        ("en", "Rename…", "Rename"),
        ("zh-Hans", "重命名…", "重命名"),
    ] {
        let (_dir, from, mut app) = fixture("origin.txt");
        let sibling = from.with_file_name("sibling.txt");
        fs::write(&sibling, "sibling").unwrap();
        app.workspace
            .open(FileLocation::path_only(sibling))
            .unwrap();
        app.run_named(None, "set_ui_language", &json!({"language":locale}));
        let point = center(tab(&mut app, 0));
        let menu = app
            .headless_context_menu(point)
            .into_iter()
            .find(|row| row.caption == menu)
            .unwrap();
        assert_eq!(menu.command.as_deref(), Some(RENAME_TAB));
        app.run_named(None, RENAME_TAB, &serde_json::from_str(&menu.args).unwrap());
        app.scene();
        let tree = app.accessible_tree();
        let field = tree.node(RENAME_FIELD).unwrap();
        assert_eq!(field.parent, Some(1000 + app.workspace.tabs[0].sheet * 2));
        assert_eq!(field.role, Role::TextField);
        assert_eq!(field.label, label);
        assert!(field.focused && field.editable);
        assert_eq!(tree.focused(), Some(RENAME_FIELD));
        assert_eq!(field.text.as_ref().unwrap().text.to_string(), "origin.txt");
        assert_eq!(value_and_selection(&app).1, "origin");
        key(&mut app, "escape");
        assert!(app.accessible_tree().node(RENAME_FIELD).is_none());
    }
}
