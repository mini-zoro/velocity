use super::*;
use std::{fs, time::SystemTime};
use vel_config::{json::Document, settings::Layer};

fn app(source: &str) -> (tempfile::TempDir, App, PathBuf) {
    let data = tempfile::tempdir().unwrap();
    let path = crate::workbench::preferences_path(data.path());
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, source).unwrap();
    let workspace = Workspace::new(
        vel_text::Text::from("alpha-beta\nsecond line\n"),
        Some(data.path().to_owned()),
        None,
        false,
    )
    .unwrap();
    let options = Options::default();
    let now = Instant::now();
    let app = App::new(
        options.clone(),
        Clock::new(now, SystemTime::now()),
        Reporter::for_options(&options, now).unwrap(),
        workspace,
        Arc::new(AtomicBool::new(false)),
        std::rc::Rc::new(std::cell::RefCell::new(crate::session::Sessions::new(None))),
    );
    (data, app, path)
}

fn key(app: &mut App, chord: &str) {
    let viewport = app.viewport();
    assert!(
        app.dispatch_key_binding(None, &Key::parse(chord).unwrap(), Instant::now(), viewport),
        "unhandled key: {chord}"
    );
}

fn type_text(app: &mut App, text: &str) {
    app.handle_text_input(
        None,
        TextInput::Insert {
            text: text.into(),
            replacement: None,
        },
    );
}

fn edit(app: &mut App, key_name: &str) {
    app.run_named(None, "show_settings", &json!({}));
    type_text(app, key_name);
    key(app, "enter"); // Search -> list
    key(app, "enter"); // List -> field or switch
}

fn json_pair(app: &mut App, path: &Path) {
    let child = app.opened_settings.pop().expect("two-pane settings editor");
    assert_eq!(child.workspace.workbench.groups.len(), 2);
    assert_eq!(child.workspace.workbench.active, 1);
    assert!(child.workspace.tabs[0].editor.read_only);
    assert_eq!(
        child.workspace.tabs[child.workspace.active].path.as_ref(),
        Some(&path.canonicalize().unwrap())
    );
}

fn click(app: &mut App, text: &str) {
    let scene = app.scene();
    let run = scene
        .layers
        .iter()
        .flat_map(|l| &l.text)
        .find(|run| run.text == text)
        .unwrap_or_else(|| panic!("missing label {text}"));
    app.handle_mouse_event(
        None,
        MouseEvent {
            position: Point {
                x: f64::from(run.x) + 1.,
                y: f64::from(run.baseline) - 4.,
            },
            button: 0,
            kind: MouseKind::Down,
            click_count: 1,
            modifiers: Default::default(),
            timestamp: 0.,
        },
    );
}

#[test]
fn settings_ui_entry_points_reuse_the_sheet_and_keep_json_pairs() {
    let (_data, mut app, path) = app("{}");
    key(&mut app, "super+,");
    assert_eq!(app.workspace.window_title(), "Settings");
    let sheet = &**app.workspace.settings_page.as_ref().unwrap() as *const Page;
    type_text(&mut app, "wrap");
    key(&mut app, "super+,");
    assert_eq!(
        sheet,
        &**app.workspace.settings_page.as_ref().unwrap() as *const Page
    );
    assert_eq!(app.text_input_state().text.to_string(), "wrap");
    click(&mut app, "Edit in JSON");
    json_pair(&mut app, &path);

    let menu = app.application_menu();
    let preferences = menu.iter().find(|row| row.id == "preferences").unwrap();
    for (caption, command) in [
        ("Settings…", "show_settings"),
        ("Settings – JSON", "edit_settings"),
    ] {
        let row = preferences
            .children
            .iter()
            .find(|row| row.caption == caption)
            .unwrap();
        assert_eq!(row.command.as_deref(), Some(command));
        app.run_named(None, command, &serde_json::from_str(&row.args).unwrap());
    }
    json_pair(&mut app, &path);

    app.run_named(
        None,
        "show_overlay",
        &json!({
            "overlay": "command_palette",
            "text": "Preferences: Settings (JSON)"
        }),
    );
    key(&mut app, "enter");
    json_pair(&mut app, &path);
    assert_eq!(
        sheet,
        &**app.workspace.settings_page.as_ref().unwrap() as *const Page
    );
}

#[test]
fn settings_ui_keyboard_coalesces_and_uses_live_editor_values() {
    let (_data, mut app, path) = app("{}");
    edit(&mut app, "line_numbers");
    assert!(!app.workspace.editor().preferences.line_numbers);
    edit(&mut app, "auto_complete_size_limit");
    let before = app.workspace.settings_write_count();
    for digit in ["1", "2", "3", "4", "5"] {
        type_text(&mut app, digit);
    }
    assert_eq!(app.workspace.settings_write_count(), before);
    assert!(app.poll_settings_input(Instant::now() + Duration::from_secs(1)));
    assert_eq!(app.workspace.settings_write_count(), before + 1);
    assert_eq!(
        app.workspace
            .editor()
            .settings
            .usize("auto_complete_size_limit", 0),
        12345
    );
    key(&mut app, "enter"); // Finishing a debounced edit does not write again.
    assert_eq!(app.workspace.settings_write_count(), before + 1);

    edit(&mut app, "word_separators");
    app.handle_text_input(
        None,
        TextInput::Marked {
            text: "分".into(),
            selected: Utf16Range::new(1, 0),
            replacement: None,
        },
    );
    assert!(app.text_input_state().marked.is_some());
    assert!(!app.poll_settings_input(Instant::now() + Duration::from_secs(1)));
    type_text(&mut app, "-字");
    key(&mut app, "enter");
    assert_eq!(app.workspace.editor().preferences.word_separators, "-字");
    app.workspace.wait_resources();
    let stored = Document::parse(&fs::read_to_string(path).unwrap()).unwrap();
    assert_eq!(stored.value["auto_complete_size_limit"], 12345);
    assert_eq!(stored.value["word_separators"], "-字");
    assert_eq!(stored.value["line_numbers"], false);
}

#[test]
fn settings_ui_reset_preserves_bytes_and_invalid_input_never_writes() {
    let user = "{\n  // editor\n  \"tab_size\"  :  8,\n\n  \"other.key\": [1,2,], // preserve\n}\n";
    let (_data, mut app, path) = app(user);
    edit(&mut app, "tab_size");
    type_text(&mut app, "bad");
    key(&mut app, "enter");
    assert!(!app.poll_settings_input(Instant::now() + Duration::from_secs(1)));
    assert_eq!(app.workspace.settings_write_count(), 0);
    assert_eq!(fs::read_to_string(&path).unwrap(), user);
    let scene = app.scene();
    assert!(
        scene
            .layers
            .iter()
            .flat_map(|l| &l.text)
            .any(|r| r.text == "Enter a whole number from 1 to 32.")
    );
    key(&mut app, "escape"); // Edit -> row.
    let tree = app.accessible_tree();
    assert!(
        tree.nodes
            .iter()
            .any(|n| n.focused && n.label == "Tab Size, tab_size, 8, Modified")
    );
    click(&mut app, "✕");
    assert!(
        app.workspace
            .settings
            .layer(Layer::User)
            .get("tab_size")
            .is_none()
    );
    assert_eq!(app.workspace.editor().preferences.tab_size, 4);
    app.workspace.wait_resources();
    assert_eq!(
        fs::read_to_string(path).unwrap(),
        user.replace("  \"tab_size\"  :  8,\n", "")
    );
    key(&mut app, "escape"); // Row -> sidebar.
    assert!(app.workspace.settings_page.is_some());
    key(&mut app, "escape"); // Sidebar -> close.
    assert!(app.workspace.settings_page.is_none());
}

#[test]
fn settings_ui_broken_file_is_read_only_and_the_repair_button_opens_json() {
    let source = "{\n // malformed\n \"line_numbers\": ,\n}\n";
    let (_data, mut app, path) = app(source);
    edit(&mut app, "line_numbers");
    assert!(
        app.workspace
            .settings_page
            .as_ref()
            .unwrap()
            .read_only()
            .unwrap()
            .starts_with("3:")
    );
    key(&mut app, "space");
    key(&mut app, "enter");
    assert!(app.workspace.editor().preferences.line_numbers);
    assert_eq!(app.workspace.settings_write_count(), 0);
    assert_eq!(fs::read_to_string(&path).unwrap(), source);
    click(&mut app, "Edit JSON to fix");
    json_pair(&mut app, &path);
    assert_eq!(fs::read_to_string(&path).unwrap(), source);
}

#[test]
fn settings_ui_metadata_keys_search_by_key_name_chinese_and_description() {
    let (_data, mut app, _) = app("{}");
    app.run_named(None, "show_settings", &json!({}));
    let page = app.workspace.settings_page.as_mut().unwrap();
    page.search_for(" ");
    let defaults = Document::parse(include_str!(
        "../resources/Packages/Default/Preferences.sublime-settings"
    ))
    .unwrap()
    .value;
    for key in defaults.as_object().unwrap().keys() {
        page.search_for(key);
        let name = page
            .results()
            .find_map(|(found, name)| (found == key).then(|| name.to_owned()))
            .unwrap_or_else(|| panic!("missing key: {key}"));
        page.search_for(&name);
        assert!(
            page.results().any(|(k, _)| k == key),
            "missing name: {name}"
        );
    }
    for query in ["字号", "Size of editor text"] {
        page.search_for(query);
        assert!(page.results().any(|(key, _)| key == "font_size"));
    }
}
