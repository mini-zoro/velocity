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
    let (_data, mut app, _) = app(
        r#"{"trim_trailing_white_space_on_save":true,"draw_indent_guides":true,"auto_complete":true,"word_wrap":true}"#,
    );
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
        let (_data, mut app, _) = app("{}");
        app.workspace.set_appearance(dark).unwrap();
        key(&mut app, "super+,");
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
        type_text(&mut app, "wrap");
        let scene = painted(&mut app, 800.);
        let text = text_ink(&scene, "wrap");
        assert!(contrast(text, settings.0) >= 4.5, "dark: {dark}");
        key(&mut app, "super+w");
        app.run_named(None, "show_panel", &json!({"panel":"find"}));
        type_text(&mut app, "wrap");
        let find = painted(&mut app, 800.);
        assert_eq!(field_style(&find, app.panels.query.bounds), settings);
        assert_eq!(text_ink(&find, "wrap"), text);
    }
}

#[test]
fn settings_ui_dense_rows_keep_tooltips_and_keyboard_json_link() {
    use vel_platform::accessibility::Role;
    let (_data, mut app, path) = app(r#"{"auto_close_tags":false}"#);
    key(&mut app, "super+,");
    let scene = painted(&mut app, 800.);
    let tree = app.accessible_tree();
    let rows: Vec<_> = tree.nodes.iter().filter(|n| n.role == Role::Row).collect();
    let whole: Vec<_> = rows
        .iter()
        .filter(|row| (44. ..=52.).contains(&row.frame.height))
        .collect();
    assert!(whole.len() >= 12);
    let first = rows[0];
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
    app.window_size.width = 510.;
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
    use vel_platform::accessibility::Role;
    let (_data, mut good, _) = app(r#"{"tab_size":4}"#);
    let (_broken_data, mut broken, _) = app("{\"tab_size\": }");
    let snapshots = [&mut good, &mut broken].map(|app| {
        key(app, "super+,");
        let scene = painted(app, 800.);
        let tree = app.accessible_tree();
        let row = tree
            .nodes
            .iter()
            .find(|n| n.role == Role::Switch && n.label == "Auto Close Tags")
            .unwrap();
        let switch_color = scene
            .layers
            .iter()
            .flat_map(|layer| &layer.rects)
            .find(|primitive| primitive.rect == row.frame)
            .unwrap()
            .color;
        (row.frame, row.enabled, row.checked, switch_color)
    });
    assert_eq!(snapshots[0].0, snapshots[1].0);
    assert!(snapshots[0].1 && !snapshots[1].1);
    assert_eq!(snapshots[0].2, snapshots[1].2);
    assert_ne!(snapshots[0].3, snapshots[1].3);
    for app in [&mut good, &mut broken] {
        type_text(app, "tab_size");
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
    assert_eq!(good.frame, broken.frame);
    assert!(good.enabled && good.editable && !broken.enabled && !broken.editable);
}

#[test]
fn settings_ui_footer_shortens_in_the_middle_and_tooltip_has_full_path() {
    let (data, mut app, path) = app("{}");
    key(&mut app, "super+,");
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
    app.window_size.width = 430.;
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
fn settings_ui_deferred_effect_is_kept_on_the_second_line() {
    let (_data, mut app, _) = app("{}");
    key(&mut app, "super+,");
    type_text(&mut app, "hot_exit");
    let scene = painted(&mut app, 800.);
    let runs: Vec<_> = scene.layers.iter().flat_map(|layer| &layer.text).collect();
    let effect = runs
        .iter()
        .find(|run| run.text == "Takes effect: Next close or quit")
        .unwrap();
    let name = runs.iter().find(|run| run.text == "Hot Exit").unwrap();
    assert!((18. ..=28.).contains(&(effect.baseline - name.baseline)));
}

#[test]
fn settings_ui_enum_uses_typed_legacy_choices_and_updates_dependencies() {
    use vel_platform::accessibility::Role;
    let (_data, mut app, path) = app("{}");
    edit(&mut app, "word_wrap");
    painted(&mut app, 800.);
    let tree = app.accessible_tree();
    let popup = tree
        .nodes
        .iter()
        .find(|n| n.role == Role::PopupButton && n.label.starts_with("Word Wrap,"))
        .unwrap();
    assert_eq!(popup.expanded, Some(true));
    assert!(tree.nodes.iter().any(|n| n.label == "On"));
    key(&mut app, "up"); // Wrap from Automatic to Off.
    key(&mut app, "enter");
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
    click(&mut app, "Reset all"); // Still safe if our own writes were queued but not on disk yet.
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
    click(&mut app, "Reset all");
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
    click(&mut app, "Reset all");
    app.workspace.wait_resources();
    assert_eq!(fs::read_to_string(path).unwrap(), before);
}

#[test]
fn settings_ui_reset_all_refuses_external_contents_even_after_another_page_write() {
    let (_data, mut app, path) = app("{}");
    key(&mut app, "super+,");
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
    click(&mut app, "Reset all");
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
