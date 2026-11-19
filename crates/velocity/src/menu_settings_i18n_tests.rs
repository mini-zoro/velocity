use super::*;
use std::{collections::BTreeSet, fs};
use vel_config::keymap::Key;

fn fixture(data: Option<&Path>) -> App {
    fixture_with_session(
        data,
        std::rc::Rc::new(std::cell::RefCell::new(crate::session::Sessions::new(None))),
        false,
    )
}

fn fixture_with_session(
    data: Option<&Path>,
    sessions: std::rc::Rc<std::cell::RefCell<crate::session::Sessions>>,
    watch: bool,
) -> App {
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
            watch,
        )
        .unwrap(),
        Arc::new(AtomicBool::new(false)),
        sessions,
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
            (None, None),
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
        user.join("Default.sublime-keymap"),
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
    let menu = app.application_menu();
    let wrap = rows(&menu)
        .find(|row| row.caption == "Wrap Selection With Tag")
        .unwrap();
    assert!(!app.command_enabled("insert_snippet", &wrap.args));
    app.workspace
        .editor_mut()
        .set_selection(vel_buffer::Selection::from_regions([
            vel_buffer::Region::new(0, 5),
            vel_buffer::Region::new(6, 6),
        ]));
    assert!(app.command_enabled("insert_snippet", &wrap.args));
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
fn round24_menu_groups_and_prompt_captions() {
    let menu = fixture(None).application_menu();
    assert_eq!(
        menu.iter().map(|item| item.id.as_str()).collect::<Vec<_>>(),
        [
            "application",
            "file",
            "edit",
            "selection",
            "find",
            "view",
            "goto",
            "tools",
            "project",
            "preferences",
            "window",
            "help"
        ]
    );
    for (group, command, caption) in [
        ("file", "split_view", "Split View"),
        ("edit", "paste_and_indent", "Paste and Indent"),
        ("selection", "find_under_expand", "Quick Add Next"),
        ("selection", "find_all_under", "Select All Matches"),
        ("find", "toggle_find_option", "In Selection"),
        ("goto", "next_view", "Next File"),
        ("goto", "prev_view", "Previous File"),
        ("goto", "next_result", "Next Result"),
        ("goto", "prev_result", "Previous Result"),
        ("tools", "new_build_system", "New Build System"),
        ("file", "revert", "Revert File…"),
        ("help", "show_indexing_status", "Indexing Status…"),
        (
            "preferences",
            "edit_syntax_settings",
            "Settings – Syntax Specific…",
        ),
        ("preferences", "customize_theme", "Customize Theme…"),
    ] {
        let matching = |row: &&MenuEntry| row.command.as_deref() == Some(command);
        assert_eq!(rows(&menu).filter(matching).count(), 1, "{command}");
        assert_eq!(
            rows(&role(&menu, group).children)
                .find(matching)
                .unwrap()
                .caption,
            caption
        );
    }
    assert!(!rows(&role(&menu, "view").children).any(|row| row.id == "tab_selection"));
    assert_eq!(
        role(&role(&menu, "selection").children, "tab_selection")
            .children
            .len(),
        6
    );
    let view = &role(&menu, "view").children;
    assert!(
        !view
            .iter()
            .any(|row| matches!(row.command.as_deref(), Some("new_pane" | "close_pane")))
    );
    assert_eq!(
        rows(view)
            .filter(|row| row.command.as_deref() == Some("close_pane"))
            .count(),
        1
    );
    fn check_dividers(menu: &[MenuEntry]) {
        assert!(menu.first().is_none_or(|row| row.caption != "-"));
        assert!(menu.last().is_none_or(|row| row.caption != "-"));
        assert!(
            menu.windows(2)
                .all(|pair| pair[0].caption != "-" || pair[1].caption != "-")
        );
        for row in menu {
            check_dividers(&row.children);
        }
    }
    check_dividers(&menu);
}

#[test]
fn round24_view_checks_and_project_requirements() {
    let data = tempfile::tempdir().unwrap();
    let mut app = fixture(Some(data.path()));
    let wrap = json!({"setting":"word_wrap"});
    let spelling = json!({"setting":"spell_check"});
    for (command, args, before) in [
        ("toggle_setting", wrap, true),
        ("toggle_setting", spelling, false),
        ("toggle_minimap", json!({}), true),
        ("toggle_tabs", json!({}), true),
        ("toggle_side_bar", json!({}), true),
        ("toggle_status_bar", json!({}), true),
    ] {
        assert_eq!(
            app.command_checked(command, &args.to_string()),
            before,
            "{command}: {args}"
        );
        app.run_named(None, command, &args);
        assert_eq!(
            app.command_checked(command, &args.to_string()),
            !before,
            "{command}: {args}"
        );
    }
    for command in [
        "edit_project",
        "close_project",
        "quick_switch_project",
        "recent_projects",
    ] {
        assert!(!app.command_enabled(command, "{}"), "{command}");
    }
    let project = data.path().join("settings.sublime-project");
    fs::write(&project, "{}").unwrap();
    app.workspace.open_project(project).unwrap();
    for command in [
        "edit_project",
        "close_project",
        "quick_switch_project",
        "recent_projects",
    ] {
        assert!(app.command_enabled(command, "{}"), "{command}");
    }
}

#[test]
fn find_in_files_where_menu_sets_scope_without_changing_the_query() {
    let mut app = fixture(None);
    app.workspace
        .translations
        .select(vel_i18n::Language::ZhHans);
    app.run_named(
        None,
        "show_panel",
        &json!({"panel":"find_in_files","text":"plain"}),
    );
    let options = app.find_options_menu(Point::default());
    assert_eq!(
        options
            .iter()
            .map(|row| row.caption.as_str())
            .collect::<Vec<_>>(),
        [
            "仅查找当前文件",
            "仅查找打开的文件",
            "仅查找打开的文件夹",
            "-",
            "保留大小写"
        ]
    );
    for (row, scope) in options
        .iter()
        .zip(["<current file>", "<open files>", "<open folders>"])
    {
        let name = row.command.as_deref().unwrap();
        assert!(app.command_enabled(name, &row.args));
        app.run_named(None, name, &serde_json::from_str(&row.args).unwrap());
        assert_eq!(app.panels.where_input.value(), scope);
        assert_eq!(app.panels.query.value(), "plain");
    }
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
    const SAVE: &str = "File: Save";
    let data = tempfile::tempdir().unwrap();
    let sessions = std::rc::Rc::new(std::cell::RefCell::new(crate::session::Sessions::new(None)));
    let mut app = fixture_with_session(Some(data.path()), sessions.clone(), true);
    let mut second = fixture_with_session(Some(data.path()), sessions, true);
    open_palette(&mut app, "save");
    open_palette(&mut second, "save");
    assert_eq!(
        palette_row(&mut app, SAVE).annotation,
        palette_row(&mut second, SAVE).annotation
    );
    let hint = palette_row(&mut app, SAVE).annotation.clone();
    assert!(!hint.is_empty());
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
    // English alias: highlight the entire shown label, not English byte positions.
    for window in [&mut app, &mut second] {
        wait_language(window, "zh-Hans");
        let row = palette_row(window, "保存");
        assert_eq!(row.annotation, hint);
        assert_eq!(row.highlights, [0, "保".len()]);
        assert!(!row.highlights.is_empty());
        assert!(
            row.highlights
                .iter()
                .all(|&at| at < row.caption.len() && row.caption.is_char_boundary(at))
        );
    }
    open_palette(&mut app, "保");
    assert_eq!(
        palette_row(&mut app, "保存").highlights,
        [0],
        "direct match stays precise"
    );
    open_palette(&mut app, "保存");
    assert_eq!(palette_row(&mut app, "保存").highlights, [0, "保".len()]);
    app.workspace.wait_resources();
    assert!(
        fs::read_to_string(
            data.path()
                .join("Packages/User/Preferences.sublime-settings")
        )
        .unwrap()
        .contains("zh-Hans")
    );
    open_palette(&mut app, "save");
    app.run_named(None, "set_ui_language", &json!({"language":"en"}));
    assert_eq!(role(&app.application_menu(), "view").caption, "View");
    assert_eq!(palette_row(&mut app, SAVE).annotation, hint);
    wait_language(&mut second, "en");
    assert_eq!(palette_row(&mut second, SAVE).annotation, hint);
    app.workspace.wait_resources();
}

fn open_palette(app: &mut App, query: &str) {
    app.run_named(
        None,
        "show_overlay",
        &json!({"overlay":"command_palette","text":query}),
    );
}

fn wait_language(app: &mut App, language: &str) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while app.workspace.global_string("ui_language", "en") != language {
        app.poll_workspace_model();
        assert!(
            Instant::now() < deadline,
            "language reload to {language} timed out"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn palette_row<'a>(app: &'a mut App, caption: &str) -> &'a vel_ui::Item {
    let deadline = Instant::now() + Duration::from_secs(10);
    while app.panels.ranking_pending() {
        let viewport = app.viewport();
        app.panels.poll(&mut app.workspace, viewport);
        assert!(
            Instant::now() < deadline,
            "No palette results for {caption}"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(app.panels.overlay, Some(crate::panels::Overlay::Palette));
    let mut hits = app
        .panels
        .quick
        .items
        .iter()
        .filter(|row| row.caption == caption);
    let result = hits
        .next()
        .unwrap_or_else(|| panic!("Missing palette row {caption}"));
    assert!(hits.next().is_none(), "Duplicate palette row {caption}");
    result
}

#[test]
fn unsupported_language_explains_the_fallback_in_the_previous_language() {
    let data = tempfile::tempdir().unwrap();
    let mut app = fixture(Some(data.path()));
    app.run_named(None, "set_ui_language", &json!({"language":"fr"}));
    assert_eq!(app.panels.message, "Unsupported UI language; using English");
    app.panels.message.clear();
    app.run_named(None, "set_ui_language", &json!({"language":"zh-Hans"}));
    assert!(
        app.panels.message.is_empty(),
        "known language should switch silently"
    );
    app.run_named(None, "set_ui_language", &json!({"language":"fr"}));
    assert_eq!(app.panels.message, "不支持此界面语言，已改用英语");
    assert_eq!(app.workspace.global_string("ui_language", "zh-Hans"), "en");
    assert_eq!(role(&app.application_menu(), "view").caption, "View");
    app.workspace.wait_resources();
    let settings = fs::read_to_string(
        data.path()
            .join("Packages/User/Preferences.sublime-settings"),
    )
    .unwrap();
    assert_eq!(
        vel_config::json::Document::parse(&settings).unwrap().value["ui_language"],
        "en"
    );
}

#[test]
fn clear_recent_items_removes_the_cached_submenu_in_both_apps() {
    let sessions = std::rc::Rc::new(std::cell::RefCell::new(crate::session::Sessions::new(None)));
    let mut a = fixture_with_session(None, sessions.clone(), false);
    let mut b = fixture_with_session(None, sessions, false);
    fn has_recent(app: &App) -> bool {
        let (_, menu) = app.menu_cache.as_ref().unwrap();
        role(menu, "file")
            .children
            .iter()
            .any(|item| item.id == "open_recent")
    }
    assert!(a.refresh_native_menu_model());
    assert!(!has_recent(&a));
    a.workspace
        .recent_files
        .extend(["/tmp/newer.txt".into(), "/tmp/older.txt".into()]);
    a.workspace.recent_folders.push("/tmp/folder".into());
    a.workspace
        .recent_projects
        .push("/tmp/example.sublime-project".into());
    assert!(a.refresh_native_menu_model());
    assert!(b.refresh_native_menu_model());
    assert!(has_recent(&a) && has_recent(&b));
    assert!(
        !b.refresh_native_menu_model(),
        "unchanged history keeps the model"
    );

    let (_, menu) = a.menu_cache.as_ref().unwrap();
    let recent = role(&role(menu, "file").children, "open_recent");
    let clear = recent
        .children
        .iter()
        .find(|i| i.command.as_deref() == Some("clear_recent_items"))
        .unwrap();
    let args = serde_json::from_str(&clear.args).unwrap();
    a.run_named(None, "clear_recent_items", &args);
    assert!(a.refresh_native_menu_model());
    assert!(b.refresh_native_menu_model());
    assert!(!has_recent(&a) && !has_recent(&b));
    assert!(a.panels.message.is_empty());
    assert!(!a.refresh_native_menu_model());
}

#[test]
fn in_selection_is_enabled_for_document_find_and_disabled_in_files() {
    let mut app = fixture(None);
    let args = json!({"option":"in_selection"});
    let enabled = |app: &App| app.command_enabled("toggle_find_option", &args.to_string());
    assert!(!enabled(&app));
    app.run_named(None, "show_panel", &json!({"panel":"find"}));
    assert!(!enabled(&app), "showing Find does not create a scope");
    app.workspace
        .editor_mut()
        .set_selection(vel_buffer::Selection::from_regions([
            vel_buffer::Region::new(0, 11),
            vel_buffer::Region::new(11, 11),
        ]));
    assert!(
        enabled(&app),
        "any non-empty region, even with a primary caret"
    );
    app.run_named(None, "toggle_find_option", &args);
    assert_eq!(app.panels.panel, Some(crate::panels::Panel::Find));
    assert!(app.command_checked("toggle_find_option", &args.to_string()));
    let files = json!({"option":"in_selection","_context_menu":"find_in_files"});
    assert!(!app.command_enabled("toggle_find_option", &files.to_string()));
    app.run_named(None, "show_panel", &json!({"panel":"find_in_files"}));
    assert!(
        !enabled(&app),
        "the Files panel does not implement a selection scope"
    );
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
fn assembled_palette_has_complete_translations_and_preserves_invocations() {
    let mut app = fixture(None);
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("resources/Packages/Default/Localization/zh-Hans.sublime-localization");
    let catalog = vel_i18n::Catalog::load(&fs::read_to_string(path).unwrap()).unwrap();
    let before = app.workspace.palette.clone();
    app.workspace
        .translations
        .select(vel_i18n::Language::ZhHans);
    let mut missing = BTreeSet::new();
    for item in &app.workspace.palette {
        let Some(localized) = app.workspace.palette_caption(item) else {
            missing.insert(&item.caption);
            continue;
        };
        assert!(
            localized
                .chars()
                .any(|ch| ('\u{4e00}'..='\u{9fff}').contains(&ch)),
            "{localized:?}"
        );
        if let Some(data) = item.caption.strip_prefix("Set Syntax: ") {
            assert_eq!(localized, format!("设置语法：{data}"));
        } else if let Some(data) = item.caption.strip_prefix("Snippet: ") {
            assert_eq!(localized, format!("代码片段：{data}"));
        } else if let Some(data) = item.caption.strip_prefix("Tools: Build System: ") {
            let shown = if data == "Automatic" { "自动" } else { data };
            assert_eq!(localized, format!("工具：构建系统：{shown}"));
        } else if !catalog.strings.contains_key(&item.caption) {
            assert_eq!(
                localized.split('：').count(),
                item.caption.split(": ").count(),
                "{item:?}"
            );
        }
    }
    assert!(
        missing.is_empty(),
        "Untranslated assembled palette rows: {missing:#?}"
    );
    assert_eq!(app.workspace.palette, before);
    assert_eq!(
        before
            .iter()
            .filter(|item| item.invocation.command == "goto_line"
                || item.caption.contains("Jump to Line"))
            .map(|item| (item.caption.as_str(), app.workspace.palette_caption(item)))
            .collect::<Vec<_>>(),
        [("Goto Line…", Some("前往行…".into()))]
    );
    for (source, translated, command) in [
        (
            "Selection: Add Selection on Previous Line",
            "选区：在上一行添加选区",
            "select_lines",
        ),
        (
            "Goto: Bookmarks: Clear Every Bookmark",
            "前往：书签：清除所有书签",
            "clear_bookmarks",
        ),
    ] {
        let item = before.iter().find(|i| i.caption == source).unwrap();
        assert_eq!(item.invocation.command, command);
        assert_eq!(
            app.workspace.palette_caption(item).as_deref(),
            Some(translated)
        );
        let hint = app.workspace.editor().keymap.hint(&item.invocation);
        open_palette(&mut app, source);
        let row = palette_row(&mut app, translated);
        assert_eq!(row.annotation, hint.unwrap_or_default());
    }
}

#[test]
fn round24_localized_menu_walk_preserves_dispatch_and_data() {
    let data = tempfile::tempdir().unwrap();
    let user = data.path().join("Packages/User");
    fs::create_dir_all(&user).unwrap();
    fs::write(user.join("Save.sublime-build"), r#"{"cmd":["true"]}"#).unwrap();
    let mut app = fixture(Some(data.path()));
    app.workspace.recent_files.push("/tmp/Save".into());
    assert!(app.workspace.refresh_file_menus());
    let english = app.application_menu();
    app.workspace
        .translations
        .select(vel_i18n::Language::ZhHans);
    let chinese = app.application_menu();

    fn walk(en: &[MenuEntry], zh: &[MenuEntry], syntax: bool, depth: usize) {
        assert_eq!(en.len(), zh.len());
        for (before, after) in en.iter().zip(zh) {
            assert_eq!(before.id, after.id);
            assert_eq!(before.command, after.command);
            assert_eq!(before.args, after.args);
            assert_eq!(before.shortcut, after.shortcut);
            if std::env::var_os("VELOCITY_R24_REVIEW").is_some() {
                eprintln!(
                    "{:indent$}{} | {}",
                    "",
                    before.caption,
                    after.caption,
                    indent = depth * 2
                );
            }
            if syntax
                || matches!(
                    before.command.as_deref(),
                    Some("open_recent_file" | "open_recent_folder" | "open_project")
                )
                || (before.command.as_deref() == Some("set_build_system")
                    && before.caption != "Automatic")
            {
                assert_eq!(before.caption, after.caption);
            } else if !(before.caption.bytes().all(|b| b.is_ascii_digit())
                || matches!(
                    before.caption.as_str(),
                    "-" | "Velocity"
                        | "Windows"
                        | "Unix"
                        | "Mac OS 9"
                        | "UTF-8"
                        | "UTF-16 LE"
                        | "UTF-16 BE"
                )
                || (before.command.as_deref() == Some("run_macro_file")
                    && !before.args.contains("Packages/Default/")))
            {
                assert!(
                    after
                        .caption
                        .chars()
                        .any(|ch| ('\u{4e00}'..='\u{9fff}').contains(&ch)),
                    "Untranslated assembled menu row: {before:?}"
                );
                assert_eq!(
                    before.caption.ends_with('…'),
                    after.caption.ends_with('…'),
                    "{before:?}"
                );
            }
            walk(
                &before.children,
                &after.children,
                syntax || before.id == "syntax_list",
                depth + 1,
            );
        }
    }
    walk(&english, &chinese, false, 0);
    assert!(rows(&chinese).any(|row| row.caption == "自动"));
    assert!(
        rows(&chinese)
            .any(|row| row.caption == "Save" && row.command.as_deref() == Some("set_build_system"))
    );
    assert!(
        rows(&chinese).any(|row| row.caption == "简体中文（GB18030）"
            && row.args.contains("Chinese Simplified (GB18030)"))
    );
    assert!(rows(&chinese).any(|row| row.caption == "在下方插入行" && row.args.contains("Add Line.sublime-macro")));
}

#[test]
fn command_icon_does_not_change_when_a_different_caption_alias_wins() {
    let mut app = fixture(None);
    open_palette(&mut app, "toggle tabs");
    let en = palette_row(&mut app, "View: Toggle Tabs").icon;
    assert_eq!(en, vel_ui::ItemIcon::View);
    app.workspace
        .translations
        .select(vel_i18n::Language::ZhHans);
    for query in ["toggle tabs", "隐藏选项卡"] {
        open_palette(&mut app, query);
        assert_eq!(palette_row(&mut app, "显示：显示或隐藏选项卡").icon, en);
    }
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
