use super::*;
use crate::{cli::FileLocation, panels::Overlay};
use std::{fs, path::Path};
use vel_buffer::{Region, Selection};
use vel_config::keymap::Key;
use vel_text::Text;
use vel_ui::workbench::Layout;

/// The settings sweep uses these fixtures for effects requiring their own
/// focused windows, filesystem state, or native-policy boundary.
pub(super) fn setting_effect(key: &str) -> Option<(&'static str, fn())> {
    macro_rules! case {
        ($test:ident) => {
            Some((stringify!($test), $test))
        };
    }
    match key {
        "open_tabs_after_current" => case!(r34_tab_insert_and_unselect_current),
        "select_across_groups" | "sidebar_on_right" => {
            case!(r34_sidebar_cross_group_and_right_geometry)
        }
        "auto_hide_tabs"
        | "auto_hide_status_bar"
        | "click_through"
        | "hide_pointer_while_typing" => case!(r34_typing_chrome_and_native_pointer_policy),
        "show_project_first" | "set_unsaved_view_name_for_syntax" => {
            case!(r34_project_title_and_unsaved_syntax_name)
        }
        "selection_description_column_type" => case!(r34_status_column_units_and_sum),
        "highlight_gutter" | "control_character_style" => {
            case!(r34_gutter_and_control_style_reach_scene)
        }
        "fold_style" | "fold_include_trailing_newline" => {
            case!(r34_fold_style_and_trailing_newline)
        }
        "hot_exit_projects" => case!(r34_hot_exit_projects_is_independent),
        "remember_layout" => case!(r34_remember_layout_with_and_without_hot_exit),
        "terminal_command" => case!(r34_terminal_uses_context_dir_and_literal_argv),
        "goto_anything_file_preview" => case!(r34_goto_file_preview_off_still_accepts_target),
        _ => None,
    }
}

fn app(text: &str, root: Option<&Path>, project: Option<PathBuf>) -> App {
    let options = Options::default();
    let now = Instant::now();
    App::new(
        options.clone(),
        Clock::new(now, std::time::SystemTime::now()),
        Reporter::for_options(&options, now).unwrap(),
        Workspace::new(Text::from(text), root.map(Path::to_owned), project, false).unwrap(),
        Arc::new(AtomicBool::new(false)),
        std::rc::Rc::new(std::cell::RefCell::new(crate::session::Sessions::new(root))),
    )
}

fn pref(app: &mut App, key: &str, value: Value) {
    app.workspace.apply_user_preference(key, Some(value));
    app.sync_active_view();
}

fn file(root: &Path, name: &str, text: &str) -> PathBuf {
    let path = root.join(name);
    fs::write(&path, text).unwrap();
    path.canonicalize().unwrap()
}

fn location(path: &Path) -> FileLocation {
    FileLocation {
        path: path.into(),
        line: 0,
        column: 0,
    }
}

fn type_text(a: &mut App, text: &str) {
    a.handle_text_input(
        None,
        TextInput::Insert {
            text: text.into(),
            replacement: None,
        },
    );
}

fn key(a: &mut App, chord: &str) {
    assert!(a.dispatch_key_binding(None, &Key::parse(chord).unwrap(), Instant::now(), 0..0));
}

#[test]
fn r34_tab_insert_and_unselect_current() {
    let root = tempfile::tempdir().unwrap();
    let mut a = app("first", None, None);
    let first = a.workspace.tabs[0].sheet;
    type_text(&mut a, "dirty");
    for _ in 0..2 {
        a.run_named(None, "new_file", &json!({}));
    }
    let appended = a.workspace.workbench.groups[0].sheets.clone();
    assert_eq!(appended[0], first);
    a.workspace.select(0);
    a.run_named(None, "new_file", &json!({}));
    assert_eq!(a.workspace.workbench.groups[0].sheets[..3], appended);

    pref(&mut a, "open_tabs_after_current", json!(true));
    a.workspace.select(0);
    let target = file(root.path(), "opened.txt", "opened");
    a.workspace.open(location(&target)).unwrap();
    let opened = a.workspace.tabs[a.workspace.active].sheet;
    assert_eq!(
        &a.workspace.workbench.groups[0].sheets[..3],
        &[first, opened, appended[1]]
    );
    a.run_named(None, "new_file", &json!({}));
    let newest = a.workspace.tabs[a.workspace.active].sheet;
    assert_eq!(a.workspace.workbench.groups[0].sheets[2], newest);

    let before = a.workspace.workbench.groups[0].sheets.clone();
    let opened_index = a.workspace.sheet_index(opened).unwrap();
    a.workspace.click_sheet(0, true, false);
    a.workspace.click_sheet(opened_index, true, false);
    assert!(a.command_enabled_args("unselect_current", &json!({})));
    a.run_named(None, "unselect_current", &json!({}));
    assert_eq!(a.workspace.workbench.groups[0].visible(), [first, newest]);
    assert_eq!(a.workspace.workbench.groups[0].sheets, before);
    a.run_named(None, "unselect_current", &json!({}));
    assert!(!a.command_enabled_args("unselect_current", &json!({})));
    a.run_named(None, "unselect_current", &json!({}));
    assert_eq!(a.workspace.workbench.groups[0].visible().len(), 1);
}

#[test]
fn r34_sidebar_cross_group_and_right_geometry() {
    for across in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let root = root.path().canonicalize().unwrap();
        let target = file(&root, "other.txt", "original text");
        let mut a = app("", None, None);
        a.workspace.open(location(&target)).unwrap();
        type_text(&mut a, "unsaved ");
        let original = a.workspace.tabs[a.workspace.active].session_document;
        a.workspace
            .workbench
            .set_layout(Layout::grid(2, 1))
            .unwrap();
        a.workspace.workbench.active = 1;
        a.workspace.focus_current_group().unwrap();
        pref(&mut a, "select_across_groups", json!(across));
        a.panels.add_folder(&mut a.workspace, root).unwrap();
        a.scene();
        let width = a.panels.side_bounds.width;
        assert_eq!(a.workbench_bounds.x, width);
        pref(&mut a, "sidebar_on_right", json!(true));
        let deadline = Instant::now() + Duration::from_secs(10);
        while !a.panels.sidebar_has_row(&target) {
            a.panels.poll(&mut a.workspace, 0..0);
            assert!(Instant::now() < deadline, "sidebar did not publish");
            std::thread::sleep(Duration::from_millis(2));
        }
        a.scene();
        let point = a.panels.test_sidebar_point(&a.workspace, &target);
        assert_eq!(a.workbench_bounds.x, 0.);
        assert_eq!(a.panels.side_bounds.x + width, a.window_size.width);
        assert!(vel_ui::contains(a.panels.side_bounds, point));
        a.handle_mouse_event(
            None,
            MouseEvent {
                kind: MouseKind::Down,
                position: point,
                button: 0,
                click_count: 2,
                modifiers: Modifiers::default(),
                timestamp: 0.,
            },
        );
        assert_eq!(a.workspace.workbench.active, usize::from(!across));
        assert_eq!(
            a.workspace.tabs[a.workspace.active].session_document,
            original
        );
        assert!(a.workspace.editor().text.to_string().contains("unsaved"));
    }
}

/// The GUI scenario: three tabs, the last current, another app frontmost, one click on the
/// first tab. The click both switches the tab and asks the platform to activate Velocity once;
/// the same click in the text only activates through AppKit and so asks for nothing.
#[test]
fn r34_first_mouse_on_a_tab_switches_and_requests_activation_once() {
    use vel_platform::first_mouse::{Down, FirstMouse, accepts};
    let mut a = app("editor", None, None);
    a.run_named(None, "new_file", &json!({}));
    a.run_named(None, "new_file", &json!({}));
    a.scene();
    assert_eq!(a.workspace.tabs.len(), 3);
    let (first, tab) = a
        .tab_hits
        .first()
        .copied()
        .expect("the first tab is painted");
    let current = a.workspace.tabs[a.workspace.active].sheet;
    assert_ne!(first, current, "the click must change the tab");
    let text = a.sheet_bounds[&current];
    let middle = |r: Rect| Point {
        x: r.x + r.width / 2.,
        y: r.y + r.height / 2.,
    };
    let inactive = |timestamp| Down {
        timestamp,
        left: true,
        command: false,
        app_active: false,
    };
    let mut answer = FirstMouse::default();
    answer.answered(accepts(&a, Some(middle(text))), 1.);
    assert!(
        !answer.take(inactive(1.)),
        "text clicks stay AppKit's activation click"
    );

    answer.answered(accepts(&a, Some(middle(tab))), 2.);
    assert!(
        answer.take(inactive(2.)),
        "the accepted tab click finishes activation"
    );
    for kind in [MouseKind::Down, MouseKind::Up] {
        a.handle_mouse_event(
            None,
            MouseEvent {
                kind,
                position: middle(tab),
                button: 0,
                click_count: 1,
                modifiers: Modifiers(0),
                timestamp: 2.,
            },
        );
    }
    assert_eq!(a.workspace.tabs[a.workspace.active].sheet, first);
    assert!(!answer.take(inactive(2.)), "one request per click");
    // Once Velocity is active, a tab click in another of its windows is AppKit's alone.
    answer.answered(accepts(&a, Some(middle(tab))), 3.);
    assert!(!answer.take(Down {
        app_active: true,
        ..inactive(3.)
    }));
}

#[test]
fn r34_typing_chrome_and_native_pointer_policy() {
    let mut a = app("editor", None, None);
    assert!(!a.accepts_first_mouse());
    assert!(!a.hide_pointer_while_typing());
    // A file tab is a one-click control: a click that activates an inactive window acts
    // on the tab it lands on, while the same click in the text only activates.
    a.scene();
    let (sheet, tab) = a
        .tab_hits
        .first()
        .copied()
        .expect("the open tab is painted");
    assert!(a.accepts_first_mouse_at(Point {
        x: tab.x + tab.width / 2.,
        y: tab.y + tab.height / 2.,
    }));
    let text = a.sheet_bounds[&sheet];
    assert!(!a.accepts_first_mouse_at(Point {
        x: text.x + text.width / 2.,
        y: text.y + text.height / 2.,
    }));
    let (_, _, button) = a.tab_button_hits.first().copied().expect("tab bar buttons");
    assert!(a.accepts_first_mouse_at(Point {
        x: button.x + button.width / 2.,
        y: button.y + button.height / 2.,
    }));
    pref(&mut a, "click_through", json!(true));
    pref(&mut a, "hide_pointer_while_typing", json!(true));
    assert!(a.accepts_first_mouse());
    assert!(a.hide_pointer_while_typing());
    pref(&mut a, "auto_hide_tabs", json!(true));
    pref(&mut a, "auto_hide_status_bar", json!(true));
    a.scene();
    let shown = a.workbench_bounds.height;
    assert!(a.tabs_visible() && a.panels.status_visible());
    a.run_named(None, "show_overlay", &json!({"overlay":"command_palette"}));
    type_text(&mut a, "abc");
    assert!(a.tabs_visible());
    key(&mut a, "escape");
    type_text(&mut a, "x");
    a.scene();
    assert!(!a.tabs_visible() && !a.panels.status_visible());
    assert!(a.tab_hits.is_empty());
    assert!(a.workbench_bounds.height > shown);
    a.handle_mouse_event(
        None,
        MouseEvent {
            kind: MouseKind::Move,
            position: Point { x: 70., y: 70. },
            button: 0,
            click_count: 0,
            modifiers: Modifiers::default(),
            timestamp: 0.,
        },
    );
    a.scene();
    assert_eq!(a.workbench_bounds.height, shown);
    assert!(a.tabs_visible() && a.panels.status_visible());
    a.run_named(None, "toggle_tabs", &json!({}));
    type_text(&mut a, "z");
    a.reveal_typing_chrome(None);
    assert!(
        !a.tabs_visible(),
        "temporary reveal must not override explicit hiding"
    );
}

#[test]
fn r34_project_title_and_unsaved_syntax_name() {
    let mut a = app("First line\nsecond", None, None);
    a.workspace.project = Some("Example.sublime-project".into());
    assert_eq!(a.workspace.window_title(), "First line — Example");
    pref(&mut a, "show_project_first", json!(true));
    assert_eq!(a.workspace.window_title(), "Example — First line");
    pref(&mut a, "set_unsaved_view_name_for_syntax", json!(false));
    assert_eq!(a.workspace.tabs[0].name(), "First line");
    a.workspace.set_file_type("Rust").unwrap();
    assert_eq!(a.workspace.tabs[0].name(), "Untitled");
    pref(&mut a, "set_unsaved_view_name_for_syntax", json!(true));
    assert_eq!(a.workspace.tabs[0].name(), "First line");
    pref(&mut a, "set_unsaved_view_name", json!(false));
    assert_eq!(a.workspace.tabs[0].name(), "Untitled");
}

#[test]
fn r34_status_column_units_and_sum() {
    let mut a = app("\t😀 0.1 -2 2.2 x", None, None);
    a.workspace.editor_mut().set_caret(5, false);
    for (style, col) in [("display", 6), ("character", 3), ("utf16", 4), ("byte", 6)] {
        pref(&mut a, "selection_description_column_type", json!(style));
        assert!(
            crate::panels::status_position(&a.workspace).starts_with(&format!("Ln 1, Col {col}"))
        );
    }
    let editor = a.workspace.editor_mut();
    editor.set_selection(Selection::from_regions([
        Region::new(6, 9),
        Region::new(10, 12),
        Region::new(13, 16),
    ]));
    assert!(crate::panels::status_position(&a.workspace).contains("Sum: 0.3"));
    a.workspace
        .editor_mut()
        .set_selection(Selection::from_regions([
            Region::new(6, 9),
            Region::new(17, 18),
        ]));
    assert!(!crate::panels::status_position(&a.workspace).contains("Sum:"));
    a.workspace
        .editor_mut()
        .set_selection(Selection::from_regions([
            Region::new(6, 9),
            Region::caret(10),
        ]));
    assert!(!crate::panels::status_position(&a.workspace).contains("Sum:"));
}

#[test]
fn r34_gutter_and_control_style_reach_scene() {
    let mut a = app("start\u{1b}end\nlast", None, None);
    pref(&mut a, "draw_control_characters", json!(true));
    pref(&mut a, "highlight_line", json!(false));
    let left = a.scene();
    let has_text = |scene: &vel_render::scene::Scene, text: &str| {
        scene
            .layers
            .iter()
            .flat_map(|l| &l.text)
            .any(|run| run.text == text)
    };
    assert!(has_text(&left, "<0x1b>"));
    pref(&mut a, "control_character_style", json!("name"));
    pref(&mut a, "highlight_gutter", json!(true));
    let highlighted = a.scene();
    assert!(has_text(&highlighted, "ESC"));
    assert!(!has_text(&highlighted, "<0x1b>"));
    assert!(highlighted.layers.iter().flat_map(|l| &l.rects).any(|r| {
        r.rect.x == a.content.x
            && r.rect.height == a.view.line_height
            && r.rect.width > 12.
            && !left
                .layers
                .iter()
                .flat_map(|l| &l.rects)
                .any(|old| old.rect == r.rect && old.color == r.color)
    }));
}

#[test]
fn r34_fold_style_and_trailing_newline() {
    let text = "fn main() {\n    let x = 1;\n    let y = 2;\n}\n";
    let mut a = app(text, None, None);
    a.workspace.set_file_type("Rust").unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !a
        .workspace
        .editor()
        .language
        .borrow()
        .as_ref()
        .is_some_and(|l| l.analysis.is_some())
    {
        a.workspace.poll();
        assert!(Instant::now() < deadline, "fold analysis was not published");
        std::thread::sleep(Duration::from_millis(2));
    }
    let close = text.find('}').unwrap();
    a.run_named(None, "fold_by_level", &json!({"level":1}));
    assert_eq!(a.workspace.editor().folded()[0].end(), close - 1);
    pref(&mut a, "fold_include_trailing_newline", json!(true));
    a.run_named(None, "fold_by_level", &json!({"level":1}));
    assert_eq!(a.workspace.editor().folded()[0].end(), close);
    pref(&mut a, "fold_style", json!("indentation"));
    a.run_named(None, "fold_by_level", &json!({"level":1}));
    assert_eq!(a.workspace.editor().folded()[0].end(), close - 1);
}

#[test]
fn r34_hot_exit_projects_is_independent() {
    for enabled in [true, false] {
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path();
        let project = file(root, "demo.sublime-project", "{}");
        let document = file(root, "base.txt", "from disk");
        let mut a = app("", Some(root), Some(project.clone()));
        a.workspace.open(location(&document)).unwrap();
        type_text(&mut a, "edited ");
        a.workspace.new_file().unwrap();
        type_text(&mut a, "unsaved buffer");
        pref(&mut a, "hot_exit_projects", json!(enabled));
        a.checkpoint(true).unwrap();
        a.sessions.borrow().flush().unwrap();
        let project_session = vel_session::load_project_workspace(&project, Some(root)).session;
        let app_session = vel_session::load(&root.join("Sessions/Session.sublime-session")).session;
        assert_eq!(app_session.documents.len(), 2);
        assert!(app_session.documents.iter().all(|d| d.dirty));
        assert_eq!(project_session.documents.len(), if enabled { 2 } else { 1 });
        assert_eq!(project_session.documents.iter().any(|d| d.dirty), enabled);
        if !enabled {
            a.close_decisions
                .push_back(tab_adapter::CloseDecision::Cancel);
            assert!(!a.prepare_window_close(None));
        }
    }
}

#[test]
fn r34_remember_layout_with_and_without_hot_exit() {
    for (hot, setting, restored) in [
        ("always", json!("auto"), 2),
        ("always", json!(false), 1),
        ("disabled", json!("auto"), 1),
        ("disabled", json!(true), 2),
    ] {
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path();
        let mut a = app("", Some(root), None);
        let left = file(root, "left.txt", "left");
        let right = file(root, "right.txt", "right");
        a.workspace.open(location(&left)).unwrap();
        a.workspace
            .workbench
            .set_layout(Layout::grid(2, 1))
            .unwrap();
        a.workspace.workbench.active = 1;
        a.workspace.open(location(&right)).unwrap();
        let focused = a.workspace.tabs[a.workspace.active].sheet;
        pref(&mut a, "hot_exit", json!(hot));
        pref(&mut a, "remember_layout", setting.clone());
        a.checkpoint(true).unwrap();
        a.sessions.borrow().flush().unwrap();
        let disk = vel_session::load(&root.join("Sessions/Session.sublime-session")).session;
        let window = &disk.windows[0];
        assert_eq!(window.workbench.groups.len(), restored, "{hot}: {setting}");
        assert_eq!(
            window.workbench.groups[window.workbench.active].focused,
            Some(focused)
        );
        assert_eq!(disk.documents.len(), 2);
        if setting == true {
            // A fresh window reads User defaults independently of the sending window.
            fs::create_dir_all(root.join("Packages/User")).unwrap();
            fs::write(
                root.join("Packages/User/Preferences.sublime-settings"),
                r#"{"remember_layout":true}"#,
            )
            .unwrap();
            let next = a.window_app(None, None, true).unwrap();
            assert_eq!(next.workspace.workbench.groups.len(), 2);
            assert_eq!(next.workspace.workbench.active, 0);
            assert_eq!(next.workspace.tabs.len(), 1);
        }
    }
}

#[test]
fn r34_terminal_uses_context_dir_and_literal_argv() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().canonicalize().unwrap();
    let unusual = root.join("子目录 ; $HOME");
    fs::create_dir(&unusual).unwrap();
    let path = file(&unusual, "doc.txt", "content");
    let mut a = app("", None, None);
    assert!(!a.command_enabled_args("open_terminal", &json!({})));
    a.workspace.open(location(&path)).unwrap();
    a.panels.add_folder(&mut a.workspace, root.clone()).unwrap();
    let default = a.terminal_launch(&json!({"dir":"$file_path"})).unwrap();
    assert_eq!(default.directory, unusual);
    assert_eq!(default.program, "/usr/bin/open");
    assert_eq!(default.arguments[1], "com.apple.Terminal");
    assert_eq!(default.arguments[3], unusual.as_os_str());
    assert_eq!(
        a.terminal_launch(&json!({"dir":"$folder"}))
            .unwrap()
            .directory,
        root
    );
    pref(
        &mut a,
        "terminal_command",
        json!(["/custom/terminal", "--directory", "$dir", "$HOME"]),
    );
    let custom = a.terminal_launch(&json!({"paths":[path]})).unwrap();
    assert_eq!(custom.program, "/custom/terminal");
    assert_eq!(custom.arguments[1], unusual.as_os_str());
    assert_eq!(custom.arguments[2], "$HOME");
    a.run_named(None, "open_terminal", &json!({"paths":[unusual]}));
    assert!(a.panels.message.is_empty());
}

#[test]
fn r34_goto_file_preview_off_still_accepts_target() {
    let fixture = tempfile::tempdir().unwrap();
    let path = file(fixture.path(), "target.txt", "alpha\nbeta\n");
    let mut a = app("origin", None, None);
    a.panels
        .add_folder(&mut a.workspace, fixture.path().canonicalize().unwrap())
        .unwrap();
    pref(&mut a, "goto_anything_file_preview", json!(false));
    a.workspace.editor_mut().set_caret(3, false);
    for accept in [false, true] {
        a.run_named(
            None,
            "show_overlay",
            &json!({"overlay":"goto","text":"target.txt:2"}),
        );
        assert_eq!(a.panels.overlay, Some(Overlay::Goto));
        let deadline = Instant::now() + Duration::from_secs(10);
        while a.panels.ranking_pending() || a.panels.quick.items.is_empty() {
            a.panels.poll(&mut a.workspace, 0..0);
            assert!(Instant::now() < deadline, "Goto results did not publish");
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(a.workspace.tabs.len(), 1);
        assert_eq!(a.workspace.editor().caret, 3);
        key(&mut a, if accept { "enter" } else { "escape" });
        assert!(a.panels.overlay.is_none());
    }
    assert_eq!(
        a.workspace.tabs[a.workspace.active].path.as_ref(),
        Some(&path)
    );
    assert!(!a.workspace.tabs[a.workspace.active].transient);
    assert_eq!(a.workspace.editor().caret, 6);
}
