use super::*;
use std::{collections::BTreeSet, fs};
use vel_config::keymap::Key;

fn fixture(data: Option<&Path>) -> App {
    let options = Options::default();
    let start = Instant::now();
    App::new(
        options.clone(),
        Clock::new(start, std::time::SystemTime::now()),
        Reporter::for_options(&options, start).unwrap(),
        Workspace::new(
            vel_text::Text::from("plain text\n"),
            data.map(Path::to_owned),
            None,
            false,
        )
        .unwrap(),
        Arc::new(AtomicBool::new(false)),
        std::rc::Rc::new(std::cell::RefCell::new(crate::session::Sessions::new(None))),
    )
}

fn role<'a>(rows: &'a [MenuEntry], id: &str) -> &'a MenuEntry {
    rows.iter()
        .find(|row| row.id == id)
        .unwrap_or_else(|| panic!("missing {id}"))
}

fn rows(menu: &[MenuEntry]) -> impl Iterator<Item = &MenuEntry> {
    fn collect<'a>(menu: &'a [MenuEntry], out: &mut Vec<&'a MenuEntry>) {
        for row in menu {
            out.push(row);
            collect(&row.children, out);
        }
    }
    let mut flat = Vec::new();
    collect(menu, &mut flat);
    flat.into_iter()
}

#[test]
fn menu_roles_and_exact_shortcuts_survive_renaming_and_reload() {
    let data = tempfile::tempdir().unwrap();
    let mut app = fixture(Some(data.path()));
    let menu = app.application_menu();
    for (caption, shortcut) in [
        ("New File", Some("super+n")),
        ("Open…", Some("super+o")),
        ("Save", Some("super+s")),
        ("Save All", Some("alt+super+s")),
        ("Find…", Some("super+f")),
        ("Replace…", Some("alt+super+f")),
        ("Goto Anything…", Some("super+p")),
        ("Goto Symbol…", Some("super+r")),
        ("Close File", Some("super+w")),
        ("Settings…", Some("super+,")),
        ("Close Window", Some("shift+super+w")),
        ("Show Side Bar", None),
        ("Quick Add Next", Some("super+d")),
    ] {
        let row = rows(&menu)
            .find(|row| row.caption == caption)
            .unwrap_or_else(|| panic!("{caption}"));
        assert_eq!(row.shortcut.as_deref(), shortcut, "{caption}");
    }
    fn rename(items: &mut [vel_config::ui::MenuItem]) {
        for item in items {
            if item.caption != "-" {
                item.caption.push_str(" [translated]");
            }
            rename(&mut item.children);
        }
    }
    rename(&mut app.workspace.menus);
    let renamed = app.application_menu();
    let syntax = role(&role(&renamed, "view").children, "syntax_list");
    assert!(rows(&syntax.children).any(|item| item.command.as_deref() == Some("set_file_type")));
    let application = role(&renamed, "application");
    assert_eq!(
        application
            .children
            .iter()
            .map(|row| (row.command.as_deref(), row.shortcut.as_deref()))
            .collect::<Vec<_>>(),
        vec![
            (Some("native_about"), None),
            (None, None),
            (Some("edit_settings"), Some("super+,")),
            (Some("native_services"), None),
            (None, None),
            (Some("native_hide"), Some("super+h")),
            (Some("native_hide_others"), Some("alt+super+h")),
            (Some("native_show_all"), None),
            (None, None),
            (Some("native_quit"), Some("super+q")),
        ]
    );
    assert_eq!(
        role(&application.children, "settings").caption,
        "Settings… [translated]"
    );
    let user = data.path().join("Packages/User");
    fs::create_dir_all(&user).unwrap();
    fs::write(
        user.join("Test.sublime-keymap"),
        r#"[{"keys":["super+alt+n"],"command":"new_file"}]"#,
    )
    .unwrap();
    app.workspace.wait_resources();
    let menu = app.application_menu();
    assert_eq!(
        rows(&menu)
            .find(|row| row.caption == "New File")
            .unwrap()
            .shortcut
            .as_deref(),
        Some("alt+super+n")
    );
}

#[test]
fn displayed_save_equivalent_still_dispatches_vintage_context() {
    let mut app = fixture(None);
    let editor = app.workspace.editor_mut();
    editor
        .settings
        .set(Layer::View, "vintage_enabled", json!(true));
    editor
        .settings
        .set(Layer::View, "vintage_start_in_command_mode", json!(true));
    editor.sync();
    assert!(editor.command_mode());
    editor.keymap.append(r#"[{"keys":["super+s"],"command":"insert","args":{"characters":"VINTAGE"},"context":[{"key":"setting.command_mode"},{"key":"vi_mode_normal"}]}]"#).unwrap();
    let menu = app.application_menu();
    let shortcut = rows(&menu)
        .find(|row| row.caption == "Save")
        .unwrap()
        .shortcut
        .as_ref()
        .unwrap();
    assert!(app.dispatch_key_binding(None, &Key::parse(shortcut).unwrap(), Instant::now(), 0..11));
    assert!(app.workspace.editor().text.to_string().contains("VINTAGE"));
    assert!(app.workspace.tabs[0].path.is_none());
}

#[test]
fn checked_and_enabled_rows_follow_editor_state() {
    let mut app = fixture(None);
    for (command, args, checked) in [
        ("set_layout", r#"{"preset":"single"}"#, true),
        ("set_layout", r#"{"preset":"columns2"}"#, false),
        ("toggle_bookmark", "{}", false),
        ("expand_tabs", r#"{"set_translate_tabs":true}"#, false),
        ("unexpand_tabs", r#"{"set_translate_tabs":true}"#, true),
    ] {
        assert_eq!(
            app.command_checked(command, args),
            checked,
            "{command} {args}"
        );
    }
    assert!(!app.command_enabled("replace_in_files", "{}"));
    assert!(app.command_enabled("save", "{}"));
    assert!(app.command_enabled("prompt_save_as", "{}"));
    app.run_named(None, "toggle_bookmark", &json!({}));
    assert!(app.command_checked("toggle_bookmark", "{}"));
    app.run_named(None, "expand_tabs", &json!({"set_translate_tabs":true}));
    assert!(app.command_checked("expand_tabs", "{}"));
    assert!(!app.command_checked("unexpand_tabs", "{}"));
    app.run_named(None, "set_layout", &json!({"preset":"columns2"}));
    assert!(app.command_checked("set_layout", r#"{"preset":"columns2"}"#));
    app.run_named(None, "toggle_record_macro", &json!({}));
    assert!(!app.command_enabled("run_macro", "{}"));
    app.workspace.pane_extent = (1., 1.);
    assert!(!app.command_enabled("new_pane", "{}"));
}

#[test]
fn enabled_menu_rows_are_safe_to_dispatch() {
    let data = tempfile::tempdir().unwrap();
    let mut app = fixture(Some(data.path()));
    let menu = app.application_menu();
    let native = [
        "native_minimize",
        "native_zoom",
        "native_bring_all_to_front",
        "native_about",
        "native_services",
        "native_hide",
        "native_hide_others",
        "native_show_all",
        "native_quit",
        "open_url",
        "prompt_open_file",
        "prompt_open_project",
        "save_project_as",
        "open_crash_reports",
        "show_indexing_status",
        "toggle_full_screen",
        "new_window",
        "close_window",
        "move_tab_to_new_window",
        "merge_all_windows",
    ];
    let mut issues = Vec::new();
    for item in rows(&menu) {
        let Some(command) = item.command.as_deref() else {
            continue;
        };
        if native.contains(&command) {
            continue;
        }
        // Use a fresh editor state for actions that affect tabs, layout, or focus.
        let mut app = fixture(Some(data.path()));
        let args = serde_json::from_str(&item.args).unwrap();
        if app.command_enabled_args(command, &args)
            && let Err(error) = app.run_named_depth(None, command, &args, 0)
        {
            issues.push(format!("{} ({command}): {error}", item.caption));
        }
    }
    assert!(issues.is_empty(), "Menu errors: {issues:#?}");
}

#[test]
fn live_language_switch_and_bilingual_palette() {
    let data = tempfile::tempdir().unwrap();
    let mut app = fixture(Some(data.path()));
    app.run_named(
        None,
        "show_overlay",
        &json!({"overlay":"command_palette","text":"save"}),
    );
    app.run_named(None, "set_ui_language", &json!({"language":"zh-Hans"}));
    let menu = app.application_menu();
    assert_eq!(role(&menu, "view").caption, "显示");
    assert_eq!(
        role(&role(&menu, "view").children, "syntax_list").caption,
        "语法"
    );
    assert_eq!(
        role(&role(&menu, "application").children, "settings").caption,
        "设置…"
    );
    assert!(app.command_checked("set_ui_language", r#"{"language":"zh-Hans"}"#));
    fn matches(app: &mut App, query: &str, highlighted: bool) {
        app.run_named(
            None,
            "show_overlay",
            &json!({"overlay":"command_palette","text":query}),
        );
        let deadline = Instant::now() + Duration::from_secs(5);
        while app.panels.ranking_pending() {
            let viewport = app.viewport();
            app.panels.poll(&mut app.workspace, viewport);
            assert!(Instant::now() < deadline, "No palette results for {query}");
            std::thread::sleep(Duration::from_millis(2));
        }
        let hits: Vec<_> = app
            .panels
            .quick
            .items
            .iter()
            .filter(|i| i.caption == "保存")
            .collect();
        assert_eq!(hits.len(), 1, "{query}");
        assert_eq!(!hits[0].highlights.is_empty(), highlighted);
    }
    matches(&mut app, "save", false);
    matches(&mut app, "保存", true);
    app.workspace.wait_resources();
    assert!(
        fs::read_to_string(
            data.path()
                .join("Packages/User/Preferences.sublime-settings")
        )
        .unwrap()
        .contains("zh-Hans")
    );
    app.run_named(None, "set_ui_language", &json!({"language":"en"}));
    assert_eq!(role(&app.application_menu(), "view").caption, "View");
    app.workspace.wait_resources();
}

#[test]
fn shipped_catalog_has_all_static_ui_captions() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/Packages/Default");
    let catalog = vel_i18n::Catalog::load(
        &fs::read_to_string(path.join("Localization/zh-Hans.sublime-localization")).unwrap(),
    )
    .unwrap();
    let mut captions = BTreeSet::new();
    for file in fs::read_dir(path).unwrap() {
        let file = file.unwrap().path();
        if !file
            .extension()
            .is_some_and(|ext| matches!(ext.to_str(), Some("sublime-menu" | "sublime-commands")))
        {
            continue;
        }
        fn collect(value: &Value, out: &mut BTreeSet<String>) {
            for entry in value.as_array().into_iter().flatten() {
                if let Some(caption) = entry["caption"].as_str()
                    && caption != "-"
                    && !caption.bytes().all(|b| b.is_ascii_digit())
                    && !vel_fileio::ENCODINGS
                        .iter()
                        .any(|(name, _)| *name == caption)
                {
                    out.insert(caption.to_owned());
                }
                collect(&entry["children"], out);
            }
        }
        let doc = vel_config::json::Document::parse(&fs::read_to_string(file).unwrap()).unwrap();
        collect(&doc.value, &mut captions);
    }
    let mut app = fixture(None);
    for row in &role(&app.application_menu(), "application").children {
        if row.caption != "-" {
            captions.insert(row.caption.clone());
        }
    }
    let missing: Vec<_> = captions
        .iter()
        .filter(|key| !catalog.strings.contains_key(*key))
        .collect();
    assert!(missing.is_empty(), "missing: {missing:?}");
}

#[test]
fn user_settings_reports_types_and_json_location() {
    let data = tempfile::tempdir().unwrap();
    let user = data.path().join("Packages/User");
    fs::create_dir_all(&user).unwrap();
    fs::write(
        user.join("Preferences.sublime-settings"),
        r#"{"font_size":"large","tab_size":false}"#,
    )
    .unwrap();
    fs::write(
        user.join("Rust.sublime-settings"),
        "{\n  \"font_size\": ?\n}",
    )
    .unwrap();
    let app = fixture(Some(data.path()));
    let reports = &app.panels.message;
    assert!(
        reports.contains(
            "font_size: expected a number, got a string; tab_size: expected a number, got a boolean"
        ),
        "{reports:?}"
    );
    assert!(
        reports.contains("Rust.sublime-settings:2:16: expected a value"),
        "{reports:?}"
    );
    assert_eq!(app.workspace.settings.number("font_size", 0.), 13.);
    let second = fixture(Some(data.path()));
    assert!(second.panels.message.is_empty());
}

#[test]
fn settings_reference_is_current() {
    let result = std::process::Command::new("python3")
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/settings-reference.py"))
        .arg("--check")
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}

#[test]
fn context_input_and_root_menus_have_distinct_targets() {
    let data = tempfile::tempdir().unwrap();
    let root = data.path().join("project");
    fs::create_dir(&root).unwrap();
    let root = root.canonicalize().unwrap();
    let mut app = fixture(Some(data.path()));
    app.run_named(None, "add_folder", &json!({"dir":root}));
    assert_eq!(
        app.sidebar_menu_kind(&root),
        crate::context_menu::ContextMenuKind::MountPoint
    );
    assert_eq!(
        app.sidebar_menu_kind(&root.join("nested")),
        crate::context_menu::ContextMenuKind::SideBar
    );
    app.run_named(None, "show_panel", &json!({"panel":"replace"}));
    app.panels.find_history.push("earlier query".into());
    app.scene();
    let field = app.panels.query.bounds;
    let popup = app.headless_context_menu(Point {
        x: field.x + 2.,
        y: field.y + 2.,
    });
    assert!(rows(&popup).any(|entry| entry.caption == "Cut"));
    let history = rows(&popup)
        .find(|entry| entry.caption == "earlier query")
        .unwrap();
    app.run_named(
        None,
        history.command.as_deref().unwrap(),
        &serde_json::from_str(&history.args).unwrap(),
    );
    assert_eq!(app.panels.query.value(), "earlier query");
    assert_eq!(app.workspace.editor().text.to_string(), "plain text\n");
}

#[test]
fn syntax_defaults_and_save_dialog_use_the_requested_setting() {
    let data = tempfile::tempdir().unwrap();
    let default = data.path().join("Packages/Example");
    fs::create_dir_all(&default).unwrap();
    fs::write(
        default.join("Plain Text.sublime-settings"),
        "{\"tab_size\": 3}",
    )
    .unwrap();
    let mut app = fixture(Some(data.path()));
    app.run_named(None, "edit_syntax_settings", &json!({}));
    let settings = app.opened_settings.last().expect("settings comparison");
    assert!(
        settings.workspace.tabs[0]
            .editor
            .text
            .to_string()
            .contains("tab_size")
    );
    assert_eq!(
        settings.workspace.tabs[1].path.as_deref(),
        Some(
            data.path()
                .canonicalize()
                .unwrap()
                .join("Packages/User/Plain Text.sublime-settings")
                .as_path()
        )
    );
    app.workspace
        .editor_mut()
        .settings
        .set(Layer::View, "default_dir", json!(default));
    assert_eq!(app.save_dialog_directory(0), Some(default));
}

#[test]
fn scene_changes_for_status_and_ui_font_settings() {
    let mut app = fixture(None);
    app.run_named(
        None,
        "show_panel",
        &json!({"panel":"find","pattern":"plain"}),
    );
    fn text_runs(app: &mut App) -> Vec<String> {
        app.scene()
            .layers
            .into_iter()
            .flat_map(|layer| layer.text)
            .map(|run| format!("{}:{:?}:{}", run.text, run.family, run.size))
            .collect()
    }
    let original = text_runs(&mut app);
    for (key, value) in [
        ("show_encoding", json!(false)),
        ("show_line_endings", json!(false)),
        ("ui_font_face", json!("Menlo")),
        ("ui_font_size", json!(18)),
    ] {
        let mut app = fixture(None);
        app.run_named(
            None,
            "show_panel",
            &json!({"panel":"find","pattern":"plain"}),
        );
        app.workspace
            .editor_mut()
            .settings
            .set(Layer::User, key, value);
        assert_ne!(original, text_runs(&mut app), "{key}");
    }
}
