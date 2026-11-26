use super::*;
use crate::app::tab_rename::RENAME_TAB;
use crate::session::Sessions;
use std::{cell::RefCell, rc::Rc};
use vel_buffer::{Region, Selection};
use vel_config::keymap::Key;
use vel_text::Text;

pub(super) fn app(text: &str) -> (tempfile::TempDir, App) {
    let dir = tempfile::tempdir().unwrap();
    let workspace = Workspace::new_with_preferred_languages(
        Text::from(text),
        Some(dir.path().to_owned()),
        false,
        &["en"],
    )
    .unwrap();
    let options = Options::default();
    let now = Instant::now();
    let app = App::new(
        options.clone(),
        Clock::new(now, std::time::SystemTime::now()),
        Reporter::for_options(&options, now).unwrap(),
        workspace,
        Arc::new(AtomicBool::new(false)),
        Rc::new(RefCell::new(Sessions::new(None))),
    );
    (dir, app)
}

fn mark(app: &mut App, text: &str, selected: Utf16Range) {
    app.handle_text_input(
        None,
        TextInput::Marked {
            text: text.into(),
            selected,
            replacement: None,
        },
    );
}

fn commit(app: &mut App, text: &str) {
    app.handle_text_input(
        None,
        TextInput::Insert {
            text: text.into(),
            replacement: None,
        },
    );
}

fn command(app: &mut App, name: &str, args: Value) {
    app.run_named_result(None, name, &args).unwrap();
}

fn key(app: &mut App, key: &str) {
    let viewport = app.viewport();
    assert!(app.dispatch_key_binding(None, &Key::parse(key).unwrap(), Instant::now(), viewport));
}

fn assert_anchor(app: &App, bounds: Rect) -> Rect {
    let state = app.text_input_state();
    let requested = state
        .marked
        .unwrap_or(Utf16Range::new(state.selected.end(), 0));
    let (rect, actual) = app.text_input_rect(requested);
    assert_eq!(actual.location, requested.location);
    assert!(rect.width > 0. && rect.height > 0., "{rect:?}");
    assert!(
        rect.x >= bounds.x
            && rect.y >= bounds.y
            && rect.x + rect.width <= bounds.x + bounds.width + 0.01
            && rect.y + rect.height <= bounds.y + bounds.height + 0.01,
        "{rect:?} in {bounds:?}"
    );
    rect
}

fn underlined(scene: &vel_render::scene::Scene, anchor: Rect) -> bool {
    scene
        .layers
        .iter()
        .flat_map(|layer| &layer.underlines)
        .any(|stroke| {
            !stroke.squiggle
                && !stroke.stippled
                && stroke.rect.x < anchor.x + anchor.width
                && stroke.rect.x + stroke.rect.width > anchor.x
                && stroke.rect.y >= anchor.y - 1.
                && stroke.rect.y <= anchor.y + anchor.height + 2.
        })
}

fn exercise_field(app: &mut App) {
    app.scene();
    let input = app.ime_field().unwrap();
    let original = input.value();
    let version = input.buffer.change_id();
    let owner = input.buffer.id();
    assert_anchor(app, input.bounds);

    mark(app, "n", Utf16Range::new(1, 0));
    let scene = app.scene();
    let field = app.ime_field().unwrap();
    assert!(field.state().text.to_string().contains('n'));
    let anchor = assert_anchor(app, field.bounds);
    assert!(underlined(&scene, anchor), "first mark at {anchor:?}");

    mark(app, "ni-e\u{301}𠮷", Utf16Range::new(5, 2));
    let state = app.text_input_state();
    assert_eq!(state.substring(state.selected).unwrap().0, "𠮷");
    assert_eq!(app.ime_field().unwrap().buffer.change_id(), version);
    assert_eq!(app.ime_field().unwrap().value(), original);
    for command in [
        Command::Down(false),
        Command::Left(false),
        Command::DeleteBackward,
        Command::Paste,
        Command::Undo,
    ] {
        assert!(app.ime_command(None, command));
        assert_eq!(app.text_input_state().selected, state.selected);
    }
    assert!(app.ime_command(None, Command::Cancel));
    assert_eq!(app.ime_field().unwrap().buffer.id(), owner);
    assert_eq!(app.ime_field().unwrap().buffer.change_id(), version);
    assert_eq!(app.ime_field().unwrap().value(), original);
    mark(app, "ni", Utf16Range::new(2, 0));
    mark(app, "", Utf16Range::default());
    app.handle_text_input(None, TextInput::Unmark);
    assert_eq!(app.ime_field().unwrap().value(), original);

    mark(app, "ni", Utf16Range::new(2, 0));
    commit(app, "你");
    app.handle_text_input(None, TextInput::Unmark);
    assert!(app.ime_field().unwrap().value().contains('你'));
    assert!(app.text_input_state().marked.is_none());
    if app.tab_rename.is_some() {
        app.tab_rename_command(None, Command::Undo);
    } else if app.settings_page_has_focus() {
        app.settings_command(None, Command::Undo, Instant::now());
    } else {
        let viewport = app.viewport();
        app.panels
            .command(&mut app.workspace, Command::Undo, viewport)
            .unwrap();
    }
    assert_eq!(app.ime_field().unwrap().value(), original);

    mark(app, "汉", Utf16Range::new(1, 0));
    assert!(app.ime_command(None, Command::Newline));
    assert!(app.ime_field().unwrap().value().contains('汉'));
    assert!(!app.ime_field().unwrap().value().contains('\n'));
    assert_eq!(
        app.ime_field().unwrap().buffer.id(),
        owner,
        "Return did not submit"
    );
}

#[test]
fn find_replace_and_find_in_files_fields() {
    for (panel, fields) in [("find", 1), ("replace", 2), ("find_in_files", 3)] {
        for field in 0..fields {
            let (_dir, mut app) = app("document text\n");
            command(&mut app, "show_panel", json!({"panel": panel}));
            for _ in 0..field {
                key(&mut app, "tab");
            }
            exercise_field(&mut app);
            assert_eq!(
                app.workspace.editor().buffer.text().to_string(),
                "document text\n"
            );
        }
    }
}

#[test]
fn palette_and_goto() {
    for options in [
        json!({"overlay": "command_palette"}),
        json!({"overlay": "goto"}),
    ] {
        let (_dir, mut app) = app("document text\n");
        command(&mut app, "show_overlay", options);
        exercise_field(&mut app);
        assert!(app.panels.overlay.is_some());
    }
}

#[test]
fn build_stdin_has_the_same_composition_boundary() {
    let (_dir, mut app) = app("sample");
    command(&mut app, "show_output_panel", json!({}));
    app.panels.configure_output(&app.workspace, None, false);
    app.panels.set_build_input(true);
    exercise_field(&mut app);
    assert_eq!(app.ime_field().unwrap().value(), "汉");
}

fn open_sidebar_input(app: &mut App, name: &str, path: &Path) {
    use crate::context_menu::{SidebarEffect, SidebarPlatform, sidebar_command};
    struct NoNativeSideEffects;
    impl SidebarPlatform for NoNativeSideEffects {
        fn trash(&mut self, _: &Path) -> Result<(), String> {
            unreachable!()
        }
        fn copy_text(&mut self, _: &str) -> Result<(), String> {
            unreachable!()
        }
        fn reveal(&mut self, _: &Path) -> Result<(), String> {
            unreachable!()
        }
    }
    let Some(SidebarEffect::Input(invocation, caption)) = sidebar_command(
        name,
        &json!({"paths": [path]}),
        &[],
        &mut NoNativeSideEffects,
    )
    .unwrap() else {
        panic!("missing sidebar field for {name}");
    };
    let viewport = app.viewport();
    assert!(
        app.panels
            .show_input_caption(
                &mut app.workspace,
                &invocation.command,
                &invocation.args,
                caption,
                viewport
            )
            .unwrap()
    );
    app.named_changed(None, false);
}

#[test]
fn inline_tab_and_sidebar_fields() {
    let (dir, mut app) = app("sample");
    let tab_file = dir
        .path()
        .canonicalize()
        .unwrap()
        .join("a long initial tab filename that fills the tab.txt");
    std::fs::write(&tab_file, "sample").unwrap();
    app.workspace
        .open(crate::cli::FileLocation::path_only(tab_file))
        .unwrap();
    app.scene();
    let sheet = app.workspace.tabs[app.workspace.active].sheet;
    command(&mut app, RENAME_TAB, json!({"_context_sheet": sheet}));
    assert!(app.tab_rename.is_some(), "inline rename opened");
    exercise_field(&mut app);
    assert!(app.tab_rename.is_some());
    app.finish_tab_rename(None);

    let existing = dir.path().join("old.txt");
    std::fs::write(&existing, "untouched").unwrap();
    open_sidebar_input(
        &mut app,
        "side_bar_rename",
        &existing.canonicalize().unwrap(),
    );
    assert!(app.panels.input().is_some(), "sidebar rename opened");
    exercise_field(&mut app);
    assert_eq!(std::fs::read_to_string(existing).unwrap(), "untouched");
    open_sidebar_input(
        &mut app,
        "side_bar_new_file",
        &dir.path().canonicalize().unwrap(),
    );
    assert!(app.panels.input().is_some(), "sidebar new file opened");
    exercise_field(&mut app);
    assert!(!dir.path().join("汉").exists());
}

#[test]
fn candidate_geometry_moves_with_scrolling_and_clips_wrap_and_right_edges() {
    use vel_config::settings::Layer;
    let original = format!(
        "{}{}\n{}",
        "above\n".repeat(25),
        "a".repeat(100),
        "below\n".repeat(25)
    );
    for (wrap, scale) in [(false, 1.), (true, 2.)] {
        let (_dir, mut app) = app(&original);
        let editor = app.workspace.editor_mut();
        editor.settings.set(Layer::View, "word_wrap", json!(wrap));
        editor.settings.set(Layer::View, "wrap_width", json!(20));
        editor.sync();
        let target = editor.text.line_to_byte(25) + 19;
        editor.set_caret(target, false);
        app.resize_views(
            Size {
                width: 440.,
                height: 330.,
            },
            scale,
        );
        app.scene();
        mark(&mut app, "n", Utf16Range::new(1, 0));
        mark(&mut app, "ni hao 中国", Utf16Range::new(1, 1));
        let scene = app.scene();
        let before = assert_anchor(&app, app.content);
        assert!(underlined(&scene, before));
        let marked = app.text_input_state().marked.unwrap();
        if wrap {
            assert!(app.text_input_rect(marked).1.length < marked.length);
        }
        app.handle_scroll_event(
            None,
            ScrollEvent {
                position: Point {
                    x: app.content.x + 100.,
                    y: app.content.y + 60.,
                },
                delta_x: 0.,
                delta_y: -app.view.line_height,
                precise: true,
                phase: 0,
                momentum_phase: 0,
                modifiers: Modifiers::default(),
                timestamp: 0.,
            },
        );
        let after = assert_anchor(&app, app.content);
        assert_ne!(after.y, before.y);
        if !wrap {
            app.finish_ime_focus(None);
            app.workspace.editor_mut().set_caret(target + 80, false);
            mark(&mut app, "ni", Utf16Range::new(2, 0));
            app.view.scroll_x = 0.; // leave the requested range beyond the right edge
            let right = assert_anchor(&app, app.content);
            assert_eq!(right.x + right.width, app.content.x + app.content.width);
        }
        assert_eq!(app.workspace.editor().buffer.text().to_string(), original);
        app.handle_text_input(None, TextInput::Discard);
        app.handle_text_input(None, TextInput::Unmark);
        assert_eq!(app.workspace.editor().text.to_string(), original);
    }
}

#[test]
fn settings_search_and_text_editor() {
    let (_dir, mut editor) = app("sample");
    command(&mut editor, "show_settings", json!({}));
    let mut settings = editor.opened_preferences.pop().unwrap();
    exercise_field(&mut settings);
    command(&mut settings, "show_settings", json!({}));
    let mut page = settings.workspace.settings_page.take().unwrap();
    page.show_pane(&mut settings.workspace, crate::settings_ui::Pane::Advanced);
    page.focus_setting("word_separators");
    settings.workspace.settings_page = Some(page);
    key(&mut settings, "enter");
    let writes = settings.workspace.settings_write_count();
    exercise_field(&mut settings);
    assert_eq!(settings.workspace.settings_write_count(), writes);
}

#[test]
fn editor_selection_focus_and_windows_are_independent() {
    let (_dir, mut app) = app("abc\nsecond\n");
    app.scene();
    let e = app.workspace.editor_mut();
    e.set_selection(Selection::from_regions([Region::new(0, 3)]));
    mark(&mut app, "n", Utf16Range::new(1, 0));
    let scene = app.scene();
    assert!(underlined(&scene, assert_anchor(&app, app.content)));
    mark(&mut app, "𠮷e\u{301}", Utf16Range::new(0, 2));
    assert_eq!(app.text_input_state().selected, Utf16Range::new(0, 2));
    assert_eq!(
        app.workspace.editor().buffer.text().to_string(),
        "abc\nsecond\n"
    );
    assert!(app.ime_command(None, Command::Newline));
    assert_eq!(
        app.workspace.editor().text.to_string(),
        "𠮷e\u{301}\nsecond\n"
    );
    app.workspace.editor_mut().command(Command::Undo);
    assert_eq!(app.workspace.editor().text.to_string(), "abc\nsecond\n");

    mark(&mut app, "editor", Utf16Range::new(6, 0));
    command(&mut app, "show_panel", json!({"panel":"replace"}));
    assert!(app.workspace.editor().marked.is_none());
    app.scene();
    mark(&mut app, "query", Utf16Range::new(5, 0));
    let query = assert_anchor(&app, app.ime_field().unwrap().bounds);
    command(
        &mut app,
        "show_overlay",
        json!({"overlay":"command_palette"}),
    );
    assert!(app.text_input_state().marked.is_none());
    app.scene();
    mark(&mut app, "palette", Utf16Range::new(7, 0));
    assert_ne!(
        query.y,
        assert_anchor(&app, app.ime_field().unwrap().bounds).y
    );
    app.finish_ime_focus(None);
    assert!(app.text_input_state().marked.is_none());
    command(&mut app, "show_panel", json!({"panel":"replace"}));
    assert!(app.text_input_state().marked.is_none());
    mark(&mut app, "shared", Utf16Range::new(6, 0));
    let id = app.panels.input().unwrap().buffer.id();
    command(&mut app, "show_panel", json!({"panel":"find"}));
    assert_eq!(app.panels.input().unwrap().buffer.id(), id);
    assert!(app.text_input_state().marked.is_none());
}

#[test]
fn tabs_click_drag_close_and_deactivation_discard_editor_preedit() {
    let (_dir, mut a) = app("first");
    let original = a.workspace.tabs[a.workspace.active].sheet;
    a.workspace.new_file().unwrap();
    a.workspace
        .activate_sheet(a.workspace.sheet_index(original).unwrap());
    a.scene();
    mark(&mut a, " ni", Utf16Range::new(3, 0));
    let other = a
        .workspace
        .tabs
        .iter()
        .find(|t| t.sheet != original)
        .unwrap()
        .sheet;
    a.workspace
        .activate_sheet(a.workspace.sheet_index(other).unwrap());
    a.named_changed(None, false);
    assert!(
        a.workspace.tabs[a.workspace.sheet_index(original).unwrap()]
            .editor
            .marked
            .is_none()
    );
    a.workspace
        .activate_sheet(a.workspace.sheet_index(original).unwrap());
    a.scene();
    mark(&mut a, "hao", Utf16Range::new(3, 0));
    let tab = a
        .tab_hits
        .iter()
        .find(|(sheet, _)| *sheet == original)
        .unwrap()
        .1;
    for (kind, dx) in [
        (MouseKind::Down, 0.),
        (MouseKind::Drag, 12.),
        (MouseKind::Up, 12.),
    ] {
        a.handle_mouse_event(
            None,
            MouseEvent {
                kind,
                button: 0,
                position: Point {
                    x: tab.x + 18. + dx,
                    y: tab.y + tab.height / 2.,
                },
                click_count: 1,
                modifiers: Modifiers::default(),
                timestamp: 0.,
            },
        );
    }
    assert!(a.workspace.editor().marked.is_none());
    mark(&mut a, "discard", Utf16Range::new(7, 0));
    let (_second_dir, mut second) = app("second");
    second.resize_views(
        Size {
            width: 510.,
            height: 315.,
        },
        2.,
    );
    second.scene();
    assert!(second.text_input_state().marked.is_none());
    a.finish_ime_focus(None); // same path used from native window deactivation/close
    mark(&mut second, "n", Utf16Range::new(1, 0));
    assert_anchor(&second, second.content);
    assert!(a.text_input_state().marked.is_none());
    assert!(a.prepare_quit(None));
    assert_eq!(a.workspace.editor().buffer.text().to_string(), "first");
}
