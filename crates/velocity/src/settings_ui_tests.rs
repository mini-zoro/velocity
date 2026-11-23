use super::*;
use crate::settings_ui::{Page, Pane};
use std::{fs, time::SystemTime};
use vel_config::{json::Document, settings::Layer};
use vel_platform::accessibility::Role;

/// An editor window with its own User preferences file.
fn editor(source: &str) -> (tempfile::TempDir, App, PathBuf) {
    let data = tempfile::tempdir().unwrap();
    let path = crate::workbench::preferences_path(data.path());
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, source).unwrap();
    let workspace = Workspace::new_with_preferred_languages(
        vel_text::Text::from("alpha-beta\nsecond line\n"),
        Some(data.path().to_owned()),
        false,
        &["en"],
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

/// Opens Settings from an editor window and returns the Settings window.
#[track_caller]
fn window(editor: &mut App) -> App {
    editor.run_named(None, "show_settings", &json!({}));
    assert!(
        editor.workspace.settings_page.is_none(),
        "an editor window never hosts the Settings page"
    );
    let settings = editor
        .opened_preferences
        .pop()
        .expect("standalone Settings window");
    assert!(settings.settings_window);
    settings
}

/// The Settings window over its own User preferences file.
fn app(source: &str) -> (tempfile::TempDir, App, PathBuf) {
    let (data, mut editor, path) = editor(source);
    let settings = window(&mut editor);
    (data, settings, path)
}

fn with_page<R>(app: &mut App, f: impl FnOnce(&mut Page, &mut Workspace) -> R) -> R {
    let mut page = app.workspace.settings_page.take().expect("Settings page");
    let result = f(&mut page, &mut app.workspace);
    app.workspace.settings_page = Some(page);
    result
}

fn show(app: &mut App, pane: Pane) {
    with_page(app, |page, workspace| page.show_pane(workspace, pane));
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

/// Activates a setting's Advanced row, as the full list shows it (key, legacy choices,
/// reset): Advanced, the row selected, then Return.
fn edit(app: &mut App, key_name: &str) {
    app.run_named(None, "show_settings", &json!({}));
    advanced(app, key_name);
    key(app, "enter"); // Row -> field, switch or popup
}

/// Shows a setting's row on Advanced with keyboard focus.
fn advanced(app: &mut App, key_name: &str) {
    show(app, Pane::Advanced);
    with_page(app, |page, _| page.focus_setting(key_name));
}

#[track_caller]
fn json_pair(app: &mut App, path: &Path) -> App {
    let child = app.opened_settings.pop().expect("two-pane settings editor");
    assert_eq!(child.workspace.workbench.groups.len(), 2);
    assert_eq!(child.workspace.workbench.active, 1);
    assert!(child.workspace.tabs[0].editor.read_only);
    assert_eq!(
        child.workspace.tabs[0]
            .editor
            .settings
            .string("velocity_settings_resource", ""),
        "Packages/Default/Preferences.sublime-settings"
    );
    assert_eq!(
        child.workspace.tabs[child.workspace.active].path.as_ref(),
        Some(&path.canonicalize().unwrap())
    );
    child
}

fn palette_settings(app: &mut App, caption: &str) {
    app.run_named(
        None,
        "show_overlay",
        &json!({"overlay": "command_palette", "text": caption}),
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    while app.panels.ranking_pending() {
        let viewport = app.viewport();
        app.panels.poll(&mut app.workspace, viewport);
        assert!(Instant::now() < deadline, "{caption} did not rank");
        std::thread::yield_now();
    }
    let row = app
        .panels
        .quick
        .items
        .iter()
        .position(|item| item.caption == caption)
        .unwrap_or_else(|| panic!("missing palette row: {caption}"));
    app.panels.quick.selected = row;
    key(app, "enter");
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

fn press(app: &mut App, node: &vel_platform::accessibility::Node) {
    let (point, _) = node.activation.expect("pressable node");
    app.handle_mouse_event(
        None,
        MouseEvent {
            position: point,
            button: 0,
            kind: MouseKind::Down,
            click_count: 1,
            modifiers: Default::default(),
            timestamp: 0.,
        },
    );
}

fn node(app: &App, role: Role, label: &str) -> vel_platform::accessibility::Node {
    app.accessible_tree()
        .nodes
        .into_iter()
        .find(|n| n.role == role && n.label == label)
        .unwrap_or_else(|| panic!("no {role:?} {label}"))
}

#[test]
fn settings_ui_entry_points_open_one_settings_window_and_keep_json_pairs() {
    let (_data, mut app, path) = editor("{}");
    // ⌘, opens the graphical Settings window, never a page inside the editor window.
    key(&mut app, "super+,");
    assert!(app.workspace.settings_page.is_none());
    assert!(!app.settings_page_has_focus());
    let mut settings = app.opened_preferences.pop().expect("Settings window");
    assert!(settings.workspace.settings_page.is_some());
    assert_eq!(settings.workspace.window_title(), "Settings");
    // The same window again: ⌘, focuses it instead of opening a second one.
    app.opened_preferences.push(settings);
    key(&mut app, "super+,");
    palette_settings(&mut app, "Preferences: Settings");
    assert_eq!(app.opened_preferences.len(), 1);
    settings = app.opened_preferences.pop().unwrap();

    let menu = app.application_menu();
    for (id, caption, command, shortcut) in [
        ("application", "Settings…", "show_settings", Some("super+,")),
        ("preferences", "Settings…", "show_settings", None),
        ("preferences", "Settings (JSON)…", "edit_settings", None),
    ] {
        let row = menu
            .iter()
            .find(|row| row.id == id)
            .unwrap()
            .children
            .iter()
            .find(|row| row.caption == caption)
            .unwrap_or_else(|| panic!("{id}: {caption}"));
        assert_eq!(row.command.as_deref(), Some(command));
        assert_eq!(row.shortcut.as_deref(), shortcut);
        if command == "edit_settings" {
            app.run_named(None, command, &serde_json::from_str(&row.args).unwrap());
            assert!(app.workspace.settings_page.is_none());
            json_pair(&mut app, &path);
        }
    }
    palette_settings(&mut app, "Preferences: Settings (JSON)");
    json_pair(&mut app, &path);
    // From the window: the General page's button and the Advanced button open the JSON pair.
    painted(&mut settings, 480.);
    click(&mut settings, "Edit Settings in JSON…");
    json_pair(&mut settings, &path);
    show(&mut settings, Pane::Advanced);
    click(&mut settings, "Edit in JSON");
    json_pair(&mut settings, &path);
    // The page survives typing and a repeated invocation inside the window.
    let sheet = &**settings.workspace.settings_page.as_ref().unwrap() as *const Page;
    type_text(&mut settings, "wrap");
    key(&mut settings, "super+,");
    assert_eq!(
        sheet,
        &**settings.workspace.settings_page.as_ref().unwrap() as *const Page
    );
    assert_eq!(settings.text_input_state().text.to_string(), "wrap");
}

#[test]
fn settings_window_lifecycle_single_instance_close_and_no_session_state() {
    let (_data, mut app, _) = editor("{}");
    let mut settings = window(&mut app);
    let id = settings.session_id;
    {
        let sessions = app.sessions.borrow();
        assert_eq!(
            sessions.settings_window.as_ref().map(|(id, _)| *id),
            Some(id)
        );
        assert!(sessions.is_live(app.session_id) && !sessions.is_live(id));
        assert_ne!(sessions.active_window, Some(id));
    }
    // It never publishes routes or snapshots, so hot exit and file routing never see it.
    settings.publish_open_route();
    settings.checkpoint(false).unwrap();
    settings.checkpoint(true).unwrap();
    assert!(!app.sessions.borrow().publishes(id));
    // Window commands stay local; document commands are disabled and ignored.
    assert!(!vel_platform::AppHandler::command_enabled(
        &settings,
        "toggle_comment",
        "{}"
    ));
    assert!(vel_platform::AppHandler::command_enabled(
        &settings, "new_file", "{}"
    ));
    assert!(vel_platform::AppHandler::command_enabled(
        &settings,
        "show_settings",
        "{}"
    ));
    let text = settings.workspace.editor().text.to_string();
    settings.run_named(None, "toggle_comment", &json!({}));
    settings.run_named(None, "insert", &json!({"characters": "x"}));
    assert_eq!(settings.workspace.editor().text.to_string(), text);
    // Esc clears the search first, then closes the window and frees the single instance.
    type_text(&mut settings, "wrap");
    key(&mut settings, "escape");
    assert!(settings.workspace.settings_page.is_some());
    assert_eq!(settings.text_input_state().text.to_string(), "");
    key(&mut settings, "escape");
    assert!(settings.workspace.settings_page.is_none());
    assert!(app.sessions.borrow().settings_window.is_none());
    // A new invocation opens a fresh window; ⌘W closes it the same way.
    let mut again = window(&mut app);
    assert_ne!(again.session_id, id);
    key(&mut again, "super+w");
    assert!(again.workspace.settings_page.is_none());
    assert!(app.sessions.borrow().settings_window.is_none());
    let mut third = window(&mut app);
    third.run_named(None, "close_window", &json!({}));
    third.run_named(None, "close", &json!({}));
    assert!(third.workspace.settings_page.is_none());
    assert!(app.sessions.borrow().settings_window.is_none());
}

#[test]
fn settings_window_quit_and_close_never_prompt_or_save() {
    let (_data, mut app, _) = editor("{}");
    let mut settings = window(&mut app);
    let id = settings.session_id;
    edit(&mut settings, "tab_size");
    type_text(&mut settings, "6");
    // Closing finishes the edit, writes it, and releases the window without session state.
    settings.close_settings_window();
    settings.workspace.wait_resources();
    assert_eq!(
        settings.workspace.settings.layer(Layer::User)["tab_size"],
        6
    );
    assert!(app.sessions.borrow().settings_window.is_none());
    assert!(!app.sessions.borrow().publishes(id));
    assert!(app.receive_preference_updates());
    assert_eq!(app.workspace.editor().preferences.tab_size, 6);
}

#[test]
fn settings_window_writes_reach_editor_windows_and_file_edits_reach_the_window() {
    let (_data, mut app, path) = editor("{}");
    let mut settings = window(&mut app);
    assert!(app.workspace.editor().preferences.line_numbers);
    show(&mut settings, Pane::page("editor"));
    painted(&mut settings, 2000.);
    let target = node(&settings, Role::CheckBox, "Show line numbers");
    press(&mut settings, &target);
    settings.publish_settings_changes();
    assert!(app.receive_preference_updates());
    assert!(!app.workspace.editor().preferences.line_numbers);
    settings.workspace.wait_resources();
    let stored = Document::parse(&fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(stored.value["line_numbers"], false);
    // An edit to the JSON file shows up in the open window.
    fs::write(&path, r#"{"line_numbers": true, "word_wrap": false}"#).unwrap();
    settings.workspace.request_resource_reload();
    settings.workspace.wait_resources();
    painted(&mut settings, 2000.);
    let check = node(&settings, Role::CheckBox, "Show line numbers");
    assert_eq!(check.checked, Some(true));
    let wrap = node(&settings, Role::RadioButton, "Wrap long lines, Off");
    assert_eq!(wrap.checked, Some(true));
}

#[test]
fn settings_pages_reference_real_keys_with_controls() {
    use crate::settings_ui::pages_for_tests as layout;
    let defaults = Document::parse(include_str!(
        "../resources/Packages/Default/Preferences.sublime-settings"
    ))
    .unwrap()
    .value;
    let metadata = Document::parse(include_str!(
        "../resources/Packages/Default/settings-ui.json"
    ))
    .unwrap()
    .value;
    let mut seen = std::collections::BTreeSet::new();
    let mut ids = std::collections::BTreeSet::new();
    let pages = layout();
    assert!((6..=9).contains(&pages.len()), "{}", pages.len());
    let (_data, mut app, _) = app("{}");
    let names = app
        .workspace
        .resources
        .matching(".png")
        .iter()
        .map(|r| r.path.clone())
        .collect::<Vec<_>>();
    for page in pages {
        assert!(ids.insert(page.id.as_str()), "duplicate page {}", page.id);
        assert!(
            names.contains(&format!("Packages/{}", page.icon)),
            "{}: missing icon {}",
            page.id,
            page.icon
        );
        assert!(!page.sections.is_empty());
        for (key, choices, enabled_when) in page.settings() {
            assert!(seen.insert(key), "{key} is on two pages");
            let meta = &metadata[key];
            assert!(
                meta.is_object(),
                "{}: {key} is not in settings-ui.json",
                page.id
            );
            assert!(defaults.get(key).is_some(), "{key} is not shipped");
            assert_ne!(meta["control"], "json_only", "{key} has no control");
            for choice in choices {
                assert!(
                    meta["choices"].as_array().unwrap().contains(choice),
                    "{key}: {choice} is not a choice"
                );
            }
            for (parent, operands) in enabled_when {
                assert!(defaults.get(&parent).is_some(), "{key}: {parent}");
                for value in operands {
                    let mut layer = serde_json::Map::from_iter([(parent.clone(), value.clone())]);
                    assert!(
                        vel_config::settings::validate_layer(
                            &mut layer,
                            defaults.as_object().unwrap()
                        )
                        .is_empty(),
                        "{key}: {parent} {value}"
                    );
                }
            }
        }
        for (command, args) in page.buttons() {
            assert!(
                matches!(
                    command,
                    "edit_settings" | "open_dir" | "customize_color_scheme" | "customize_theme"
                ),
                "{}: {command}",
                page.id
            );
            if command == "edit_settings" {
                assert!(args["base_file"].as_str().is_some());
            }
        }
    }
    // Every curated page opens and draws its controls without raw keys.
    let pages: Vec<_> = (0..layout().len()).map(Pane::Page).collect();
    for pane in pages {
        show(&mut app, pane);
        let scene = painted(&mut app, 480.);
        let texts: Vec<_> = scene
            .layers
            .iter()
            .flat_map(|l| &l.text)
            .map(|run| run.text.clone())
            .collect();
        for key in &seen {
            assert!(!texts.contains(&(*key).to_owned()), "{pane:?} shows {key}");
        }
    }
}

#[test]
fn settings_search_covers_every_setting_from_any_page() {
    let (_data, mut app, _) = app("{}");
    let defaults = Document::parse(include_str!(
        "../resources/Packages/Default/Preferences.sublime-settings"
    ))
    .unwrap()
    .value;
    for pane in [Pane::page("files"), Pane::page("packages"), Pane::Advanced] {
        show(&mut app, pane);
        let page = app.workspace.settings_page.as_mut().unwrap();
        for key in defaults.as_object().unwrap().keys() {
            page.search_for(key);
            if key == "wrap_width" || key == "wrap_width_style" {
                continue; // Hidden while word wrap is off, as everywhere.
            }
            if ["trim_only_modified_white_space", "indent_guide_options"].contains(&key.as_str())
                || key == "auto_complete_commit_on_tab"
            {
                continue;
            }
            assert!(page.results().any(|(k, _)| k == key), "{pane:?}: {key}");
        }
        page.search_for("");
        assert_eq!(page.pane(), pane);
    }
}

#[test]
fn settings_keyboard_tab_space_arrows_and_popups() {
    let (_data, mut app, path) = app("{}");
    painted(&mut app, 480.);
    // Search, then the category list: arrows switch pages.
    key(&mut app, "tab");
    key(&mut app, "down");
    key(&mut app, "down");
    assert_eq!(
        app.workspace.settings_page.as_ref().unwrap().pane(),
        Pane::page("editor")
    );
    key(&mut app, "up");
    key(&mut app, "down");
    key(&mut app, "down");
    assert_eq!(
        app.workspace.settings_page.as_ref().unwrap().pane(),
        Pane::page("files")
    );
    // Tab enters the page: Encoding field, Line endings popup, Trim popup, then checkboxes.
    painted(&mut app, 480.);
    for _ in 0..4 {
        key(&mut app, "tab");
    }
    painted(&mut app, 480.);
    let tree = app.accessible_tree();
    let focused = tree.node(tree.focused().unwrap()).unwrap();
    assert_eq!(focused.role, Role::CheckBox, "{}", focused.label);
    assert_eq!(focused.label, "End files with a newline");
    assert_eq!(focused.checked, Some(false));
    key(&mut app, "space");
    painted(&mut app, 480.);
    assert_eq!(
        node(&app, Role::CheckBox, "End files with a newline").checked,
        Some(true)
    );
    // Shift+Tab back to the Line endings popup; Enter opens it, Down and Enter choose.
    key(&mut app, "shift+tab");
    key(&mut app, "shift+tab");
    painted(&mut app, 480.);
    let tree = app.accessible_tree();
    let focused = tree.node(tree.focused().unwrap()).unwrap();
    assert_eq!(focused.role, Role::PopupButton, "{}", focused.label);
    assert!(focused.label.starts_with("Line endings,"));
    key(&mut app, "enter");
    painted(&mut app, 480.);
    assert_eq!(
        node(&app, Role::PopupButton, "Line endings, System Default").expanded,
        Some(true)
    );
    key(&mut app, "down");
    key(&mut app, "enter");
    app.workspace.wait_resources();
    let stored = Document::parse(&fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(stored.value["default_line_ending"], "unix");
    assert_eq!(stored.value["ensure_newline_at_eof_on_save"], true);
    // Radios move with Left and Right.
    show(&mut app, Pane::page("sidebar"));
    with_page(&mut app, |page, _| page.focus_setting("file_tab_style"));
    key(&mut app, "right");
    assert_eq!(
        app.workspace.settings.layer(Layer::User)["file_tab_style"],
        "square"
    );
    key(&mut app, "left");
    key(&mut app, "left");
    assert_eq!(
        app.workspace.settings.layer(Layer::User)["file_tab_style"],
        "rounded"
    );
    // ⌘F returns to the search field; typing in a list also searches.
    key(&mut app, "super+f");
    type_text(&mut app, "minimap");
    assert!(
        app.workspace
            .settings_page
            .as_ref()
            .unwrap()
            .results()
            .any(|(key, _)| key == "show_minimap")
    );
}

#[test]
fn settings_font_popup_lists_families_on_demand_and_type_selects() {
    let (_data, mut app, _) = app(r#"{"font_face": "Menlo"}"#);
    show(&mut app, Pane::page("editor"));
    with_page(&mut app, |page, _| page.focus_setting("font_face"));
    painted(&mut app, 480.);
    key(&mut app, "enter"); // No window to ask: only the current face is listed.
    painted(&mut app, 480.);
    assert_eq!(
        node(&app, Role::PopupButton, "Font, Menlo").expanded,
        Some(true)
    );
    key(&mut app, "escape");
    with_page(&mut app, |page, workspace| {
        page.provide_fonts(
            workspace,
            ["Monaco", "Courier New", "Menlo", "Courier"]
                .map(str::to_owned)
                .to_vec(),
        )
    });
    key(&mut app, "enter");
    type_text(&mut app, "cou");
    type_text(&mut app, "rier n");
    key(&mut app, "enter");
    assert_eq!(
        app.workspace.settings.layer(Layer::User)["font_face"],
        "Courier New"
    );
}

#[test]
fn settings_window_accessibility_roles() {
    let (_data, mut app, _) = app("{}");
    painted(&mut app, 480.);
    let tree = app.accessible_tree();
    let search = tree
        .nodes
        .iter()
        .find(|n| n.role == Role::SearchField)
        .unwrap();
    assert!(search.focused && search.editable && search.label == "Search settings");
    let categories = tree
        .nodes
        .iter()
        .find(|n| n.role == Role::List && n.label == "Settings categories")
        .unwrap();
    let rows: Vec<_> = tree
        .nodes
        .iter()
        .filter(|n| n.parent == Some(categories.id))
        .collect();
    assert_eq!(rows.len(), crate::settings_ui::pages_for_tests().len() + 1);
    assert!(rows.iter().all(|row| row.role == Role::Row));
    assert!(rows[0].selected && rows[0].label == "General");
    assert_eq!(rows.last().unwrap().label, "Advanced");
    node(&app, Role::Heading, "Startup");
    let reopen = node(&app, Role::CheckBox, "Reopen files from the last session");
    assert_eq!(reopen.checked, Some(true));
    assert!(reopen.help.contains("Takes effect"));
    node(&app, Role::PopupButton, "Interface language, Automatic");
    node(&app, Role::Button, "Edit Settings in JSON…");
    show(&mut app, Pane::page("sidebar"));
    painted(&mut app, 480.);
    assert_eq!(
        node(&app, Role::RadioButton, "Tab shape, Angled").checked,
        Some(true)
    );
    assert_eq!(
        node(&app, Role::RadioButton, "Tab shape, Rounded").checked,
        Some(false)
    );
    // Pressing a category through accessibility shows it.
    let advanced = app
        .accessible_tree()
        .nodes
        .into_iter()
        .find(|n| n.role == Role::Row && n.label == "Advanced")
        .unwrap();
    press(&mut app, &advanced);
    painted(&mut app, 480.);
    node(&app, Role::PopupButton, "Group, All settings");
    node(&app, Role::CheckBox, "Show modified only");
}

#[test]
fn settings_search_accepts_ime_composition() {
    let (_data, mut app, _) = app(r#"{"ui_language": "zh-Hans"}"#);
    app.handle_text_input(
        None,
        TextInput::Marked {
            text: "huan".into(),
            selected: Utf16Range::new(4, 0),
            replacement: None,
        },
    );
    assert!(app.text_input_state().marked.is_some());
    type_text(&mut app, "换行");
    assert!(app.text_input_state().marked.is_none());
    let page = app.workspace.settings_page.as_ref().unwrap();
    assert!(page.results().any(|(key, _)| key == "word_wrap"));
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
    type_text(&mut app, "tab");
    key(&mut app, "escape"); // Clears the search.
    assert!(app.workspace.settings_page.is_some());
    key(&mut app, "escape"); // Closes the window.
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
    let defaults = Document::parse(include_str!(
        "../resources/Packages/Default/Preferences.sublime-settings"
    ))
    .unwrap()
    .value;
    let fixtures = ["en", "zh-Hans"].map(|language| {
        app(&json!({
            "ui_language": language, "trim_trailing_white_space_on_save":true,
            "draw_indent_guides":true, "auto_complete":true,"word_wrap":true
        })
        .to_string())
    });
    let copies = fixtures.each_ref().map(|(_, app, _)| {
        app.workspace
            .settings_page
            .as_ref()
            .unwrap()
            .localized_copy()
            .map(|(key, name, description)| {
                (key.to_owned(), (name.to_owned(), description.to_owned()))
            })
            .collect::<std::collections::BTreeMap<_, _>>()
    });
    for (language, (_data, mut app, _)) in ["en", "zh-Hans"].into_iter().zip(fixtures) {
        let page = app.workspace.settings_page.as_mut().unwrap();
        for key in defaults.as_object().unwrap().keys() {
            let (en_name, en_desc) = &copies[0][key];
            let (zh_name, zh_desc) = &copies[1][key];
            assert_ne!(en_name, zh_name, "{key}");
            for query in [key, en_name, zh_name, en_desc, zh_desc] {
                page.search_for(query);
                assert!(
                    page.results().any(|(k, _)| k == key),
                    "{language}: {key}: {query}"
                );
            }
        }
        page.search_for("字号");
        assert!(page.results().any(|(key, _)| key == "font_size"));
    }
}

#[test]
fn settings_ui_descriptions_exclude_developer_notes_and_paths() {
    let languages = std::iter::once("en".to_owned())
        .chain(crate::ui_language::shipped_catalogs().map(|(tag, _)| tag.as_str().to_owned()));
    for language in languages {
        let (_data, app, _) = app(&json!({"ui_language": language}).to_string());
        let page = app.workspace.settings_page.as_ref().unwrap();
        for (key, _, description) in page.localized_copy() {
            let text = description.to_lowercase();
            assert!(
                !text.trim().is_empty()
                    && !text.contains(['\n', '\r', '/', '\\'])
                    && !["lane ", "round ", "http", "todo"]
                        .iter()
                        .any(|word| text.contains(word))
                    && !text.split_whitespace().any(|word| {
                        word.trim_matches(|ch: char| !ch.is_ascii_alphanumeric())
                            .rsplit_once('.')
                            .is_some_and(|(_, ext)| ext.bytes().any(|ch| ch.is_ascii_alphabetic()))
                    }),
                "{language} {key}: {description}"
            );
        }
    }
    for source in [
        include_str!("../resources/Packages/Default/Preferences.sublime-settings"),
        include_str!("../resources/Packages/Default/Preferences (OSX).sublime-settings"),
    ] {
        for line in source
            .lines()
            .filter_map(|line| line.trim().strip_prefix("//"))
        {
            let comment = line.to_lowercase();
            assert!(
                !["lane ", "round ", "http://", "https://"]
                    .iter()
                    .any(|word| comment.contains(word)),
                "{line}"
            );
        }
    }
}

fn painted(app: &mut App, height: f64) -> vel_render::scene::Scene {
    app.window_size = Size {
        width: 900.,
        height,
    };
    app.scene()
}

fn contrast(a: [f32; 4], b: [f32; 4]) -> f32 {
    let linear = |c: f32| {
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    let y = |c: [f32; 4]| 0.2126 * linear(c[0]) + 0.7152 * linear(c[1]) + 0.0722 * linear(c[2]);
    let (a, b) = (y(a), y(b));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

fn field_style(scene: &vel_render::scene::Scene, frame: Rect) -> ([f32; 4], [f32; 4]) {
    use vel_render::scene::Surface;
    let surfaces = scene.layers.iter().flat_map(|layer| &layer.surfaces);
    let fill = surfaces
        .clone()
        .filter_map(|surface| match surface {
            Surface::Fill(fill) if fill.rect == frame && fill.color[3] == 1. => Some(fill.color),
            _ => None,
        })
        .next_back()
        .expect("opaque themed field");
    let border = surfaces
        .filter_map(|surface| match surface {
            Surface::CssBox(css) if css.rect == frame && css.borders[0].width > 0. => {
                Some(css.borders[0].color)
            }
            _ => None,
        })
        .next_back()
        .expect("focus border");
    (fill, border)
}

fn text_ink(scene: &vel_render::scene::Scene, text: &str) -> [f32; 4] {
    scene
        .layers
        .iter()
        .flat_map(|layer| &layer.text)
        .find(|run| run.text == text)
        .unwrap_or_else(|| panic!("not painted: {text}"))
        .color
}

#[test]
fn settings_ui_search_uses_find_field_style_and_passes_wcag_contrast() {
    for dark in [true, false] {
        let (_data, mut editor, _) = editor("{}");
        editor.workspace.set_appearance(dark).unwrap();
        let mut app = window(&mut editor);
        let scene = painted(&mut app, 800.);
        let search = app.accessible_tree().focused().unwrap();
        let bounds = app.accessible_tree().node(search).unwrap().frame;
        let settings = field_style(&scene, bounds);
        assert_ne!(settings.0, settings.1);
        assert!(
            contrast(text_ink(&scene, "Search settings"), settings.0) >= 4.5,
            "dark: {dark}"
        );
        let field_layer = scene
            .layers
            .iter()
            .find(|layer| {
                layer.surfaces.iter().any(|surface|
            matches!(surface, vel_render::scene::Surface::Fill(fill) if fill.rect == bounds))
            })
            .unwrap();
        let center = Point {
            x: bounds.x + bounds.width / 2.,
            y: bounds.y + bounds.height / 2.,
        };
        // Rectangles composite above surfaces, so a filled pseudo-focus ring here would hide the input.
        assert!(
            !field_layer
                .rects
                .iter()
                .any(|rect| vel_ui::contains(rect.rect, center))
        );
        // Labels, section headings, hints and descriptions meet WCAG AA on the window.
        let background = scene.background;
        for label in [
            "Reopen files from the last session",
            "Startup",
            "Takes effect: Next close or quit",
            "General",
        ] {
            assert!(
                contrast(text_ink(&scene, label), background) >= 4.5,
                "dark: {dark}: {label}"
            );
        }
        type_text(&mut app, "wrap");
        let scene = painted(&mut app, 800.);
        let text = text_ink(&scene, "wrap");
        assert!(contrast(text, settings.0) >= 4.5, "dark: {dark}");
        let description = "Disable automatic wrapping at this file size in bytes.";
        for text in [description, "Editor › Word Wrap", "More in Advanced"] {
            assert!(
                contrast(text_ink(&scene, text), scene.background) >= 4.5,
                "dark: {dark}: {text}"
            );
        }
        editor.run_named(None, "show_panel", &json!({"panel":"find"}));
        type_text(&mut editor, "wrap");
        let find = painted(&mut editor, 800.);
        assert_eq!(field_style(&find, editor.panels.query.bounds), settings);
        assert_eq!(text_ink(&find, "wrap"), text);
    }
}

#[test]
fn settings_ui_dense_rows_keep_tooltips_and_keyboard_json_link() {
    let (_data, mut app, path) = app(r#"{"auto_close_tags":false}"#);
    show(&mut app, Pane::Advanced);
    with_page(&mut app, |page, _| page.show_group_named("Editing"));
    let scene = painted(&mut app, 800.);
    let tree = app.accessible_tree();
    let rows: Vec<_> = tree.nodes.iter().filter(|n| n.role == Role::Row).collect();
    let whole: Vec<_> = rows
        .iter()
        .filter(|row| (44. ..=52.).contains(&row.frame.height))
        .collect();
    assert!(whole.len() >= 10, "{}", whole.len());
    let first = rows
        .iter()
        .copied()
        .find(|row| {
            row.parent
                != tree
                    .nodes
                    .iter()
                    .find(|n| n.label == "Settings categories")
                    .map(|n| n.id)
        })
        .unwrap();
    assert!(first.label.contains("auto_close_tags") && first.label.ends_with("Modified"));
    assert!(
        tree.nodes
            .iter()
            .any(|n| n.parent == Some(first.id) && n.label == "Reset setting")
    );
    let labels: Vec<_> = scene.layers.iter().flat_map(|layer| &layer.text).collect();
    let name = labels
        .iter()
        .find(|run| run.text == "Auto Close Tags")
        .unwrap();
    let setting_key = labels
        .iter()
        .find(|run| run.text == "auto_close_tags")
        .unwrap();
    assert!((name.baseline - setting_key.baseline).abs() < 3.);
    assert_eq!(setting_key.family.as_deref(), Some("Menlo"));
    assert_eq!(
        labels
            .iter()
            .filter(|run| run.text == "Edit in JSON")
            .count(),
        1
    ); // Toolbar only.
    assert!(
        !labels
            .iter()
            .any(|run| run.text.contains("Takes effect: Immediately"))
    );
    app.window_size.width = 560.;
    let narrow = app.scene();
    let ellipsized = narrow
        .layers
        .iter()
        .flat_map(|layer| &layer.text)
        .any(|run| {
            run.text
                .strip_suffix('…')
                .is_some_and(|prefix| !prefix.is_empty() && first.help.starts_with(prefix))
        });
    assert!(
        ellipsized,
        "the full description should not wrap into another row"
    );
    painted(&mut app, 800.);
    let p = Point {
        x: first.frame.x + 20.,
        y: first.frame.y + 12.,
    };
    let page = app.workspace.settings_page.as_mut().unwrap();
    page.hover(p, Instant::now() - Duration::from_secs(1));
    let tip = painted(&mut app, 800.);
    let tip_text = tip
        .layers
        .last()
        .unwrap()
        .text
        .iter()
        .map(|run| run.text.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    assert_eq!(tip_text, first.help);
    assert!(
        app.accessible_tree()
            .nodes
            .iter()
            .any(|n| n.role == Role::Link && n.parent == Some(first.id))
    );
    with_page(&mut app, |page, workspace| page.focus_search(workspace));
    key(&mut app, "enter");
    key(&mut app, "tab");
    painted(&mut app, 800.);
    let tree = app.accessible_tree();
    assert_eq!(tree.node(tree.focused().unwrap()).unwrap().role, Role::Link);
    key(&mut app, "enter");
    json_pair(&mut app, &path);
}

#[test]
fn settings_ui_broken_controls_keep_geometry_and_are_dimmed() {
    let (_data, mut good, _) = app(r#"{"tab_size":4}"#);
    let (_broken_data, mut broken, _) = app("{\"tab_size\": }");
    let snapshots = [&mut good, &mut broken].map(|app| {
        advanced(app, "auto_close_tags");
        let scene = painted(app, 800.);
        let tree = app.accessible_tree();
        let row = tree
            .nodes
            .iter()
            .find(|n| n.role == Role::CheckBox && n.label == "Auto Close Tags")
            .unwrap();
        let fill = scene
            .layers
            .iter()
            .flat_map(|layer| &layer.surfaces)
            .find_map(|surface| match surface {
                vel_render::scene::Surface::CssBox(css)
                    if vel_ui::contains(
                        row.frame,
                        Point {
                            x: css.rect.x + 1.,
                            y: css.rect.y + 1.,
                        },
                    ) && css.rect.width < 24. =>
                {
                    Some(css.background)
                }
                _ => None,
            })
            .unwrap();
        (row.frame, row.enabled, row.checked, fill)
    });
    assert_eq!(snapshots[0].0, snapshots[1].0);
    assert!(snapshots[0].1 && !snapshots[1].1);
    assert_eq!(snapshots[0].2, snapshots[1].2);
    assert_ne!(snapshots[0].3, snapshots[1].3);
    for app in [&mut good, &mut broken] {
        advanced(app, "tab_size");
        painted(app, 800.);
    }
    let field = |app: &App| {
        app.accessible_tree()
            .nodes
            .into_iter()
            .find(|n| n.role == Role::TextField && n.label == "Tab Size")
            .unwrap()
    };
    let (good, broken) = (field(&good), field(&broken));
    // The read-only notice makes the list shorter, so only the row's scroll position differs.
    let shape = |r: Rect| (r.x, r.width, r.height);
    assert_eq!(shape(good.frame), shape(broken.frame));
    assert!(good.enabled && good.editable && !broken.enabled && !broken.editable);
}

#[test]
fn settings_ui_footer_shortens_in_the_middle_and_tooltip_has_full_path() {
    let (data, mut app, path) = app("{}");
    show(&mut app, Pane::Advanced);
    app.workspace
        .settings_page
        .as_mut()
        .unwrap()
        .use_fixture_home(data.path().parent().unwrap().to_owned());
    let wide = painted(&mut app, 800.);
    assert!(
        wide.layers
            .iter()
            .flat_map(|l| &l.text)
            .any(|run| run.text.starts_with("User file: ~/"))
    );
    app.window_size.width = 470.;
    let narrow = app.scene();
    let footer = narrow
        .layers
        .iter()
        .flat_map(|l| &l.text)
        .find(|run| run.text.starts_with("User file:"))
        .unwrap();
    assert!(footer.text.contains("~/") && footer.text.contains('…'));
    assert!(
        footer.text.ends_with("Preferences.sublime-settings"),
        "{}",
        footer.text
    );
    app.workspace.settings_page.as_mut().unwrap().hover(
        Point {
            x: f64::from(footer.x) + 1.,
            y: f64::from(footer.baseline) - 4.,
        },
        Instant::now() - Duration::from_secs(1),
    );
    let tooltip = app.scene();
    let tooltip_text = tooltip
        .layers
        .last()
        .unwrap()
        .text
        .iter()
        .map(|run| run.text.as_str())
        .collect::<String>();
    assert_eq!(tooltip_text, path.to_string_lossy());
}

#[test]
fn settings_ui_advanced_rows_keep_description_and_effect_clear_of_controls() {
    let (_data, mut app, _) = app("{}");
    show(&mut app, Pane::Advanced);
    for width in [720., 600.] {
        app.window_size = Size {
            width,
            height: 2400.,
        };
        let scene = app.scene();
        let runs: Vec<_> = scene.layers.iter().flat_map(|layer| &layer.text).collect();
        let run = |text: &str| {
            *runs
                .iter()
                .find(|run| {
                    run.text == text
                        || run.text.trim_end_matches('…').len() > 3
                            && text.starts_with(run.text.trim_end_matches('…'))
                })
                .unwrap_or_else(|| panic!("{width}: missing {text}"))
        };
        let tree = app.accessible_tree();
        let control = |label: &str| {
            tree.nodes
                .iter()
                .find(|n| n.label.starts_with(label) && n.role != Role::Row)
                .unwrap_or_else(|| panic!("{width}: no control {label}"))
                .frame
        };
        // A list's add field stays in the control column, right of the description.
        let description = run("Treat these extra words as valid spelling.");
        let field = control("Added Words, Add tag…");
        assert!(f64::from(description.x) < field.x);
        let words = theme_width(&app, description);
        assert!(f64::from(description.x) + words <= field.x - 8., "{width}");
        // The effect note has its own line, below a description that is not squeezed.
        let atomic = run("Write to a temporary file and replace the original when possible.");
        let effect = runs
            .iter()
            .find(|run| {
                run.text == "Takes effect: Next save"
                    && (run.baseline - atomic.baseline).abs() < 40.
            })
            .expect("effect note");
        assert!(effect.baseline - atomic.baseline >= 14., "{width}");
        assert!((f64::from(effect.x) - f64::from(atomic.x)).abs() < 1.);
        let check = control("Atomic Save");
        assert!(
            f64::from(atomic.x) + theme_width(&app, atomic) <= check.x,
            "{width}"
        );
    }
}

/// The drawn advance of a run, measured with the window's own text metrics.
fn theme_width(app: &App, run: &vel_render::scene::TextRun) -> f64 {
    use vel_theme::{Element, ElementPath};
    let mut theme = app.workspace.themes.ui.borrow_mut();
    let mut style = theme.style(
        crate::panels::StyleId::SettingsLabel.at(0),
        &ElementPath(&[
            Element::new("window", &[]),
            Element::new("label_control", &[]),
        ]),
        &app.workspace.settings,
    );
    style.size = run.size;
    style.family = run.family.clone();
    style.font = run.style;
    theme.advance(&style, &run.text)
}

#[test]
fn settings_ui_number_fields_share_one_stepper() {
    let (_data, mut app, _) = app("{}");
    for pane in [Pane::page("editor"), Pane::Advanced] {
        show(&mut app, pane);
        if pane == Pane::Advanced {
            advanced(&mut app, "tab_size");
        }
        let scene = painted(&mut app, 900.);
        let texts: Vec<_> = scene
            .layers
            .iter()
            .flat_map(|l| &l.text)
            .map(|r| r.text.as_str())
            .collect();
        assert!(
            !texts.contains(&"−") && !texts.contains(&"+"),
            "{pane:?}: {texts:?}"
        );
        let tree = app.accessible_tree();
        let field = tree
            .nodes
            .iter()
            .find(|n| {
                n.role == Role::TextField && ["Tab width", "Tab Size"].contains(&n.label.as_str())
            })
            .unwrap()
            .frame;
        let beside = |label: &str| {
            tree.nodes
                .iter()
                .find(|n| {
                    n.label == label
                        && n.frame.y >= field.y - 1.
                        && n.frame.y + n.frame.height <= field.y + field.height + 1.
                        && n.frame.x > field.x
                })
                .unwrap()
                .clone()
        };
        let (up, down) = (beside("Increase"), beside("Decrease"));
        // One stepper: the halves of the same chevron button, increase on top.
        assert_eq!(up.frame.x, down.frame.x);
        assert!(up.frame.y < down.frame.y);
        assert!((up.frame.y + up.frame.height - down.frame.y).abs() < 0.5);
        let target = up;
        let before = app.workspace.editor().settings.usize("tab_size", 0);
        press(&mut app, &target);
        assert!(app.poll_settings_input(Instant::now() + Duration::from_secs(1)));
        assert_eq!(
            app.workspace.editor().settings.usize("tab_size", 0),
            before + 1
        );
    }
}

#[test]
fn settings_search_shows_curated_matches_first_and_jumps_to_their_page() {
    for (language, query, location, more) in [
        ("en", "wrap", "Editor › Word Wrap", "More in Advanced"),
        ("zh-Hans", "换行", "编辑器 › 自动换行", "高级中的更多结果"),
    ] {
        let (_data, mut app, _) = app(&json!({"ui_language": language}).to_string());
        type_text(&mut app, query);
        let scene = painted(&mut app, 1600.);
        let runs: Vec<_> = scene.layers.iter().flat_map(|l| &l.text).collect();
        let caption = runs
            .iter()
            .find(|r| r.text == location)
            .expect("location caption");
        let group = runs
            .iter()
            .find(|r| r.text == more)
            .expect("Advanced group");
        assert!(caption.baseline < group.baseline);
        // Curated results show no keys; the Advanced group below may.
        let keys: Vec<_> = runs
            .iter()
            .filter(|r| r.family.as_deref() == Some("Menlo") && r.text.contains('_'))
            .collect();
        assert!(
            keys.iter().all(|r| r.baseline > group.baseline),
            "{language}"
        );
        assert!(!keys.is_empty());
        let page = app.workspace.settings_page.as_ref().unwrap();
        let results: Vec<_> = page.results().map(|(key, _)| key).collect();
        assert_eq!(
            results.first(),
            Some(&"word_wrap"),
            "{language}: {results:?}"
        );
        // Activating a curated result edits it in place, with its curated control.
        let radio = app
            .accessible_tree()
            .nodes
            .into_iter()
            .find(|n| n.role == Role::RadioButton && n.checked == Some(false))
            .unwrap();
        press(&mut app, &radio);
        assert!(
            app.workspace
                .settings
                .layer(Layer::User)
                .contains_key("word_wrap")
        );
        // The caption shows the option on its page, focused and in view.
        painted(&mut app, 1600.);
        let link = app
            .accessible_tree()
            .nodes
            .into_iter()
            .find(|n| n.role == Role::Link && n.label == location)
            .unwrap();
        press(&mut app, &link);
        let page = app.workspace.settings_page.as_ref().unwrap();
        assert_eq!(page.pane(), Pane::page("editor"));
        assert_eq!(page.results().next().map(|(key, _)| key), Some("font_face"));
        painted(&mut app, 480.);
        let tree = app.accessible_tree();
        let focused = tree.node(tree.focused().unwrap()).unwrap();
        assert_eq!(focused.role, Role::RadioButton, "{language}");
        assert!(focused.frame.y >= 0. && focused.frame.y + focused.frame.height <= 480.);
    }
}

#[test]
fn settings_descriptions_name_keys_and_system_for_the_platform() {
    use crate::settings_ui::platform_text_for;
    let drag = "Move selected text by dragging. Hold {alt} to copy it.";
    assert_eq!(
        platform_text_for(drag, true),
        "Move selected text by dragging. Hold Alt to copy it."
    );
    assert_eq!(
        platform_text_for(drag, false),
        "Move selected text by dragging. Hold Option to copy it."
    );
    assert_eq!(platform_text_for("{command}+{os}", true), "Ctrl+Windows");
    assert_eq!(platform_text_for("{command}+{os}", false), "Command+macOS");
    // Every description, in English and in each catalog, names keys through the tokens.
    let metadata = Document::parse(include_str!(
        "../resources/Packages/Default/settings-ui.json"
    ))
    .unwrap()
    .value;
    let catalogs: Vec<_> = crate::ui_language::shipped_catalogs().collect();
    let words = regex_lite_words();
    for (key, meta) in metadata.as_object().unwrap() {
        let source = meta["description"].as_str().unwrap();
        let mut texts = vec![source.to_owned()];
        texts.extend(
            catalogs
                .iter()
                .filter_map(|(_, catalog)| catalog.strings.get(source).cloned()),
        );
        for text in texts {
            for windows in [true, false] {
                let resolved = platform_text_for(&text, windows);
                assert!(!resolved.contains('{'), "{key}: {resolved}");
                if windows {
                    for word in &words {
                        assert!(
                            !resolved
                                .split(|c: char| !c.is_alphanumeric() && c != '⌘' && c != '⌥')
                                .any(|w| w == *word),
                            "{key}: {resolved} names a Mac key on Windows"
                        );
                    }
                }
            }
        }
    }
    // The window shows the platform's wording, in both languages.
    for (language, wanted) in [("en", "Hold "), ("zh-Hans", "按住 ")] {
        let (_data, app, _) = app(&json!({"ui_language": language}).to_string());
        let page = app.workspace.settings_page.as_ref().unwrap();
        let (_, _, description) = page
            .localized_copy()
            .find(|(key, _, _)| *key == "drag_text")
            .unwrap();
        let name = if cfg!(windows) { "Alt" } else { "Option" };
        assert!(
            description.contains(&format!("{wanted}{name}")),
            "{language}: {description}"
        );
    }
}

/// Mac-only key names that must not appear in a Windows description.
fn regex_lite_words() -> [&'static str; 5] {
    ["Option", "Command", "Cmd", "⌘", "⌥"]
}

#[test]
fn settings_ui_enum_uses_typed_legacy_choices_and_updates_dependencies() {
    let (_data, mut app, path) = app("{}");
    edit(&mut app, "word_wrap");
    let scene = painted(&mut app, 800.);
    assert_eq!(
        scene
            .layers
            .iter()
            .flat_map(|layer| &layer.text)
            .filter(|run| run.text == "Edit in JSON")
            .count(),
        1,
        "only the header button is drawn while the enum is open"
    );
    let tree = app.accessible_tree();
    let popup = tree
        .nodes
        .iter()
        .find(|n| n.role == Role::PopupButton && n.label.starts_with("Word Wrap,"))
        .unwrap();
    assert_eq!(popup.expanded, Some(true));
    assert!(!tree.nodes.iter().any(|n| n.role == Role::Link));
    assert!(tree.nodes.iter().any(|n| n.label == "On"));
    key(&mut app, "up"); // Wrap from Automatic to Off.
    key(&mut app, "enter");
    painted(&mut app, 800.);
    assert!(
        app.accessible_tree()
            .nodes
            .iter()
            .any(|n| n.role == Role::Link)
    );
    assert_eq!(
        app.workspace.settings.layer(Layer::User)["word_wrap"],
        false
    );
    app.workspace
        .settings_page
        .as_mut()
        .unwrap()
        .search_for("wrap");
    let page = app.workspace.settings_page.as_ref().unwrap();
    assert!(
        !page
            .results()
            .any(|(key, _)| ["wrap_width", "wrap_width_style"].contains(&key))
    );
    painted(&mut app, 800.);
    let mut announcements = vel_platform::accessibility::Announcements::default();
    assert_eq!(
        announcements.changes(&app.accessible_tree()),
        ["Word Wrap, off"]
    );
    assert!(announcements.changes(&app.accessible_tree()).is_empty());
    key(&mut app, "super+z");
    assert!(
        !app.workspace
            .settings
            .layer(Layer::User)
            .contains_key("word_wrap")
    );
    let page = app.workspace.settings_page.as_ref().unwrap();
    assert_eq!(
        page.results()
            .filter(|(key, _)| ["wrap_width", "wrap_width_style"].contains(key))
            .count(),
        2
    );
    edit(&mut app, "hot_exit");
    click(&mut app, "Off");
    app.workspace.wait_resources();
    let stored = Document::parse(&fs::read_to_string(path).unwrap()).unwrap();
    assert_eq!(stored.value["hot_exit"], false);
    assert!(stored.value.get("word_wrap").is_none());
}

#[test]
fn settings_ui_curated_dependencies_hide_and_dim() {
    let (_data, mut app, _) = app(r#"{"word_wrap": false, "vintage_enabled": false}"#);
    show(&mut app, Pane::page("editor"));
    painted(&mut app, 2000.);
    let tree = app.accessible_tree();
    // show_when hides the column while wrapping is off; enabled_when dims sub-options.
    assert!(!tree.nodes.iter().any(|n| n.label == "Wrap column"));
    let indent = node(&app, Role::CheckBox, "Indent wrapped lines");
    assert!(!indent.enabled);
    let target = node(&app, Role::RadioButton, "Wrap long lines, On");
    press(&mut app, &target);
    painted(&mut app, 2000.);
    assert!(node(&app, Role::TextField, "Wrap column").enabled);
    assert!(node(&app, Role::CheckBox, "Indent wrapped lines").enabled);
    show(&mut app, Pane::page("keys"));
    painted(&mut app, 2000.);
    let start = node(&app, Role::CheckBox, "Start in command mode");
    assert!(!start.enabled && start.activation.is_none());
    app.handle_mouse_event(
        None,
        MouseEvent {
            position: Point {
                x: start.frame.x + 8.,
                y: start.frame.y + start.frame.height / 2.,
            },
            button: 0,
            kind: MouseKind::Down,
            click_count: 1,
            modifiers: Default::default(),
            timestamp: 0.,
        },
    );
    assert!(
        !app.workspace
            .settings
            .layer(Layer::User)
            .contains_key("vintage_start_in_command_mode")
    );
}

#[test]
fn settings_ui_curated_buttons_run_their_commands() {
    let (_data, mut app, _) = app("{}");
    show(&mut app, Pane::page("keys"));
    painted(&mut app, 480.);
    let target = node(&app, Role::Button, "Key Bindings…");
    press(&mut app, &target);
    let keys = app.opened_settings.pop().expect("key bindings pair");
    let resource = keys.workspace.tabs[0]
        .editor
        .settings
        .string("velocity_settings_resource", "")
        .to_owned();
    assert!(resource.ends_with(".sublime-keymap"), "{resource}");
}

#[test]
fn settings_ui_arrays_add_remove_and_open_overflow_without_losing_types() {
    let (_data, mut app, path) = app(
        r#"{"file_exclude_patterns":["*.log","*.bak","*.o","*.pyc","*.tmp"],"rulers":[80,[100,"dotted"]]}"#,
    );
    edit(&mut app, "file_exclude_patterns");
    let scene = painted(&mut app, 800.);
    text_ink(&scene, "*.log");
    text_ink(&scene, "+2 more");
    type_text(&mut app, "*.swp");
    key(&mut app, "enter");
    click(&mut app, "+3 more");
    click(&mut app, "×  *.tmp");
    assert_eq!(
        app.workspace.settings.layer(Layer::User)["file_exclude_patterns"],
        json!(["*.log", "*.bak", "*.o", "*.pyc", "*.swp"])
    );
    edit(&mut app, "rulers");
    type_text(&mut app, "not a column");
    key(&mut app, "enter");
    text_ink(&painted(&mut app, 800.), "Enter a positive column number.");
    key(&mut app, "super+a");
    type_text(&mut app, "120");
    key(&mut app, "enter");
    app.workspace.wait_resources();
    assert_eq!(
        Document::parse(&fs::read_to_string(path).unwrap())
            .unwrap()
            .value["rulers"],
        json!([80, [100, "dotted"], 120])
    );
}

#[test]
fn settings_ui_array_chips_fit_short_labels_and_cap_long_ones() {
    let long_pattern = format!("{}*.log", "long-pattern-".repeat(12));
    let (_data, mut app, _) = app(&json!({
        "ui_language": "zh-Hans",
        "indent_guide_options": ["draw_normal", "draw_active"],
        "file_exclude_patterns": [long_pattern]
    })
    .to_string());
    edit(&mut app, "indent_guide_options");
    let scene = painted(&mut app, 800.);
    text_ink(&scene, "draw_normal");
    text_ink(&scene, "draw_active");

    edit(&mut app, "file_exclude_patterns");
    let scene = painted(&mut app, 800.);
    assert!(
        scene
            .layers
            .iter()
            .flat_map(|layer| &layer.text)
            .any(|run| {
                run.text
                    .strip_suffix('…')
                    .is_some_and(|prefix| !prefix.is_empty() && long_pattern.starts_with(prefix))
            })
    );
}

/// Reset all lives on the Advanced page.
fn reset_all(app: &mut App) {
    show(app, Pane::Advanced);
    click(app, "Reset all");
}

#[test]
fn settings_ui_reset_all_restores_the_exact_opening_file() {
    let source = "{\n  // opening comment\n  \"tab_size\": 5,\n  \"unknown\": [true, false],\n}\n";
    let (_data, mut app, path) = app(source);
    edit(&mut app, "line_numbers");
    edit(&mut app, "tab_size");
    type_text(&mut app, "7");
    key(&mut app, "enter");
    key(&mut app, "super+z");
    assert_eq!(app.workspace.settings.layer(Layer::User)["tab_size"], 5);
    assert!(!app.workspace.editor().preferences.line_numbers);
    reset_all(&mut app); // Still safe if our own writes were queued but not on disk yet.
    app.workspace.wait_resources();
    painted(&mut app, 800.);
    assert_eq!(fs::read_to_string(path).unwrap(), source);
    assert!(app.workspace.editor().preferences.line_numbers);
}

#[test]
fn settings_ui_reset_all_checks_mtime_and_offers_reload() {
    let (_data, mut app, path) = app("{}");
    edit(&mut app, "line_numbers");
    app.workspace.wait_resources();
    painted(&mut app, 800.);
    let before = fs::read_to_string(&path).unwrap();
    // The guard runs on the serial queue, even if the resource watcher has not fired yet.
    let (unblock, blocked) = std::sync::mpsc::channel::<()>();
    let blocker =
        crate::release::enqueue(move || blocked.recv().map_err(|e| e.to_string())).unwrap();
    reset_all(&mut app);
    let file = fs::File::open(&path).unwrap();
    file.set_modified(file.metadata().unwrap().modified().unwrap() + Duration::from_secs(5))
        .unwrap();
    unblock.send(()).unwrap();
    blocker.recv().unwrap().unwrap();
    app.workspace.wait_resources();
    text_ink(
        &painted(&mut app, 800.),
        "The file was changed outside Settings",
    );
    assert_eq!(fs::read_to_string(&path).unwrap(), before);
    click(&mut app, "Reload");
    app.workspace.wait_resources();
    edit(&mut app, "line_numbers");
    reset_all(&mut app);
    app.workspace.wait_resources();
    assert_eq!(fs::read_to_string(path).unwrap(), before);
}

#[test]
fn settings_ui_reset_all_refuses_external_contents_even_after_another_page_write() {
    let (_data, mut app, path) = app("{}");
    let modified = fs::metadata(&path).unwrap().modified().unwrap();
    fs::write(&path, r#"{"tab_size":9}"#).unwrap();
    fs::File::open(&path)
        .unwrap()
        .set_modified(modified)
        .unwrap();
    // An unrelated page edit can still merge with the new document.
    edit(&mut app, "line_numbers");
    app.workspace.wait_resources();
    let after = fs::read_to_string(&path).unwrap();
    let values = Document::parse(&after).unwrap().value;
    assert_eq!(values["tab_size"], 9);
    assert_eq!(values["line_numbers"], false);
    reset_all(&mut app);
    app.workspace.wait_resources();
    text_ink(
        &painted(&mut app, 800.),
        "The file was changed outside Settings",
    );
    assert_eq!(fs::read_to_string(path).unwrap(), after);
}

#[test]
fn settings_ui_external_revision_keeps_conflicting_draft_and_updates_other_rows() {
    let (_data, mut app, path) = app("{}");
    edit(&mut app, "tab_size");
    type_text(&mut app, "11");
    fs::write(&path, r#"{"tab_size":8,"line_numbers":false}"#).unwrap();
    app.workspace.wait_resources();
    painted(&mut app, 800.);
    assert_eq!(app.text_input_state().text.to_string(), "11");
    assert!(!app.workspace.editor().preferences.line_numbers);
    text_ink(
        &painted(&mut app, 800.),
        "This value changed outside Settings.",
    );
    assert!(!app.poll_settings_input(Instant::now() + Duration::from_secs(1)));
    key(&mut app, "enter");
    assert_eq!(app.workspace.settings_write_count(), 0);
    assert_eq!(app.workspace.settings.layer(Layer::User)["tab_size"], 8);
    key(&mut app, "escape");
    key(&mut app, "enter");
    assert_eq!(app.text_input_state().text.to_string(), "8");
}

#[test]
fn settings_ui_queued_conflict_does_not_overwrite_disk_and_retains_submitted_draft() {
    let (_data, mut app, path) = app("{}");
    let (unblock, blocked) = std::sync::mpsc::channel::<()>();
    let blocker =
        crate::release::enqueue(move || blocked.recv().map_err(|e| e.to_string())).unwrap();
    edit(&mut app, "tab_size");
    type_text(&mut app, "11");
    key(&mut app, "enter"); // Submit and leave the field while the worker is blocked.
    fs::write(&path, r#"{"tab_size":8}"#).unwrap();
    unblock.send(()).unwrap();
    blocker.recv().unwrap().unwrap();
    app.workspace.wait_resources();
    painted(&mut app, 800.);
    assert_eq!(app.workspace.settings.layer(Layer::User)["tab_size"], 8);
    assert_eq!(fs::read_to_string(path).unwrap(), r#"{"tab_size":8}"#);
    key(&mut app, "enter");
    assert_eq!(app.text_input_state().text.to_string(), "11");
    assert_eq!(app.workspace.settings_write_count(), 1);
}
