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
        Workspace::new_with_preferred_languages(
            vel_text::Text::from("plain text\n"),
            data.map(Path::to_owned),
            watch,
            &["en"],
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
        .select(vel_i18n::Language::parse("zh-Hans").unwrap());
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
            "保留大小写",
            "遵循 .gitignore"
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
    let row = options.last().unwrap();
    let name = row.command.as_deref().unwrap();
    assert!(app.command_enabled(name, &row.args));
    assert!(app.command_checked(name, &row.args));
    app.run_named(None, name, &serde_json::from_str(&row.args).unwrap());
    assert!(!app.command_checked(name, &row.args));
    let mut session = vel_session::Window::default();
    app.panels.capture_session(&mut session);
    let session = serde_json::from_slice(&serde_json::to_vec(&session).unwrap()).unwrap();
    let mut restored = crate::panels::Panels::new(&app.workspace, false);
    restored.restore_session(&app.workspace, &session);
    assert!(!restored.find_option_checked("use_gitignore"));
    restored.restore_session(&app.workspace, &serde_json::from_str("{}").unwrap());
    assert!(restored.find_option_checked("use_gitignore"));
    assert_eq!(app.panels.query.value(), "plain");
}

#[test]
fn print_exports_current_selection_or_document_and_reaps_the_browser_file() {
    use std::cell::RefCell;
    use vel_buffer::{Region, Selection};
    #[derive(Default)]
    struct Browser(RefCell<Vec<PathBuf>>);
    impl vel_platform::BrowserOpener for Browser {
        fn open_browser_file(&self, path: &Path) -> bool {
            assert!(path.is_file());
            self.0.borrow_mut().push(path.to_owned());
            true
        }
    }
    let mut app = fixture(None);
    app.workspace.set_file_type("Rust").unwrap();
    r25_language(&mut app, "zh-Hans");
    let menu = app.application_menu();
    let item = rows(&menu)
        .find(|r| r.command.as_deref() == Some("print"))
        .unwrap();
    assert_eq!(item.caption, "打印…");
    assert!(app.command_enabled("print", &item.args));
    open_palette(&mut app, "print");
    let _ = palette_row(&mut app, "文件：打印…");
    app.run_named(None, "hide_overlay", &json!({}));
    let source = "fn main() {\n    // 猫 <&>\n}\n";
    let editor = app.workspace.editor_mut();
    editor.run("select_all", &json!({})).unwrap();
    editor
        .run("insert", &json!({"characters": source}))
        .unwrap();
    let background = editor
        .language
        .borrow()
        .as_ref()
        .unwrap()
        .scheme
        .default_style()
        .background;
    let browser = Browser::default();
    for selection_only in [true, false] {
        if selection_only {
            let start = source.find("//").unwrap();
            app.workspace
                .editor_mut()
                .set_selection(Selection::from_regions([Region::new(
                    start,
                    source.rfind('}').unwrap(),
                )]));
        } else {
            app.workspace.editor_mut().set_caret(0, false);
        }
        app.start_print().unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(result) = app.print_jobs.poll(&browser, Instant::now()) {
                result.unwrap();
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(1));
        }
        let path = browser.0.borrow_mut().pop().unwrap();
        assert_eq!(path.extension().unwrap(), "html");
        let html = fs::read_to_string(&path).unwrap();
        assert!(html.contains("window.print()"));
        assert!(html.contains("charset=\"utf-8\"") && html.contains("猫 &lt;&amp;&gt;"));
        assert!(html.contains("<span class=\"s"));
        let (_, body) = html.split_once("</head>").unwrap();
        assert_eq!(body.contains("main"), !selection_only);
        assert!(html.contains(&format!(
            "background-color:#{:02x}{:02x}{:02x}",
            background.r, background.g, background.b
        )));
        assert!(path.exists());
        app.print_jobs
            .poll(&browser, Instant::now() + Duration::from_secs(601));
        assert!(!path.exists());
    }
}

#[test]
fn folder_picker_request_mounts_only_in_a_new_workspace() {
    let data = tempfile::tempdir().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let mut app = fixture(Some(data.path()));
    let menu = app.application_menu();
    let row = rows(&menu)
        .find(|row| row.command.as_deref() == Some("prompt_open_folder_in_new_window"))
        .unwrap();
    assert!(app.command_enabled("prompt_open_folder_in_new_window", &row.args));
    open_palette(&mut app, "open folder in new window");
    let _ = palette_row(&mut app, "File: Open Folder in New Window…");
    assert!(App::folder_window_request(None).unwrap().is_none());
    let request = App::folder_window_request(Some(dir.path().canonicalize().unwrap()))
        .unwrap()
        .unwrap();
    let new = app.app_for_new_window(request).unwrap();
    assert_eq!(new.workspace.folders.len(), 1);
    assert_eq!(
        new.workspace.folders[0].path,
        dir.path().canonicalize().unwrap()
    );
    assert!(app.workspace.folders.is_empty());
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
        "prompt_open_folder_in_new_window",
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
fn default_ui_language_follows_the_first_supported_system_preference() {
    for (preferred, expected) in [
        (&["fr-CA", "zh-Hans-CN", "en-US"][..], "zh-Hans"),
        (&["en-GB", "zh-Hans"], "en"),
        (&["fr-CA", "zh-TW", "zh-Hans"], "zh-Hans"),
        (&["fr-CA", "zh-TW"], "en"),
        (&[][..], "en"),
    ] {
        let workspace =
            Workspace::new_with_preferred_languages(vel_text::Text::new(), None, false, preferred)
                .unwrap();
        assert_eq!(
            workspace.settings.layer(Layer::Default)["ui_language"],
            "auto"
        );
        assert_eq!(workspace.ui_language_setting(), "auto");
        assert_eq!(workspace.checked_ui_language(), "auto");
        assert_eq!(
            workspace.translations.matched_language().as_str(),
            expected,
            "{preferred:?}"
        );
        assert!(workspace.ui_language_notice.is_none());
    }

    let data = tempfile::tempdir().unwrap();
    let path = crate::workbench::preferences_path(data.path());
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, r#"{"ui_language":"en"}"#).unwrap();
    let mut workspace = Workspace::new_with_preferred_languages(
        vel_text::Text::new(),
        Some(data.path().to_owned()),
        false,
        &["zh-Hans"],
    )
    .unwrap();
    assert_eq!(workspace.checked_ui_language(), "en");
    assert!(workspace.translations.language().is_english());
    workspace
        .set_user_preference("ui_language", json!("auto"))
        .unwrap();
    workspace.wait_resources();
    assert_eq!(workspace.checked_ui_language(), "auto");
    assert_eq!(
        workspace.translations.matched_language().as_str(),
        "zh-Hans"
    );
}

#[test]
fn live_language_switch_and_bilingual_palette() {
    const SAVE: &str = "File: Save";
    const ZH_SAVE: &str = "文件：保存";
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
        let row = palette_row(window, ZH_SAVE);
        assert_eq!(row.annotation, hint);
        assert_eq!(
            row.highlights,
            ZH_SAVE.char_indices().map(|(at, _)| at).collect::<Vec<_>>()
        );
    }
    open_palette(&mut app, "保");
    assert_eq!(
        palette_row(&mut app, ZH_SAVE).highlights,
        ["文件：".len()],
        "direct match stays precise"
    );
    open_palette(&mut app, "保存");
    assert_eq!(
        palette_row(&mut app, ZH_SAVE).highlights,
        ["文件：".len(), "文件：保".len()]
    );
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
    while app.workspace.ui_language_setting() != language {
        app.poll_workspace_model();
        assert!(
            Instant::now() < deadline,
            "language reload to {language} timed out"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn r25_language(app: &mut App, language: &str) {
    app.run_named(None, "set_ui_language", &json!({"language":language}));
    assert_eq!(app.workspace.ui_language_setting(), language);
}

fn r25_paint(app: &mut App, wanted: &[&str]) {
    let scene = app.scene();
    let runs: Vec<_> = scene.layers.iter().flat_map(|layer| &layer.text).collect();
    let theme = app.workspace.themes.ui.borrow();
    for text in wanted {
        let run = runs
            .iter()
            .find(|run| run.text == *text)
            .unwrap_or_else(|| {
                panic!(
                    "missing {text:?} in {:?}",
                    runs.iter().map(|r| &r.text).collect::<Vec<_>>()
                )
            });
        let ink = vel_minihtml::TextStyle {
            font_family: run.family.clone().unwrap_or_else(|| "monospace".into()),
            font_size: run.size,
            font_weight: if run.style.bold { 700 } else { 400 },
            italic: run.style.italic,
            underline: false,
            color: vel_minihtml::Color::rgb(0, 0, 0),
        };
        let width =
            vel_minihtml::FontMetrics::measure(theme.metrics.as_ref(), &run.text, &ink).advance;
        assert!(
            f64::from(run.x + width) <= run.clip.x + run.clip.width + 1.,
            "{run:?}"
        );
    }
}

#[test]
fn r25_find_family_and_where_menu_relabel_in_place() {
    let data = tempfile::tempdir().unwrap();
    let mut app = fixture(Some(data.path()));
    for scale in [1., 2.] {
        app.workspace
            .editor_mut()
            .settings
            .set(Layer::View, "ui_scale", json!(scale));
        for (panel, english, chinese) in [
            (
                "find",
                &["Find", "Find Prev", "Find All"][..],
                &["查找", "查找上一个", "查找全部"][..],
            ),
            (
                "incremental_find",
                &["Find", "Find Prev"][..],
                &["查找", "查找上一个"][..],
            ),
            (
                "replace",
                &["Find", "Replace", "Replace All"][..],
                &["查找", "替换", "全部替换"][..],
            ),
            (
                "find_in_files",
                &["Find", "Replace", "Where"][..],
                &["查找", "替换", "位置"][..],
            ),
        ] {
            app.run_named(None, "show_panel", &json!({"panel":panel}));
            let opened = app.panels.panel;
            r25_paint(&mut app, english);
            r25_language(&mut app, "zh-Hans");
            r25_paint(&mut app, chinese);
            assert_eq!(app.panels.panel, opened);
            let ax = app.accessible_tree();
            assert!(ax.nodes.iter().any(|node| node.label == "正则表达式"));
            if panel == "find_in_files" {
                let menu = app.find_options_menu(Point::default());
                assert!(rows(&menu).any(|m| m.caption == "仅查找打开的文件夹"));
            }
            r25_language(&mut app, "en");
            r25_paint(&mut app, english);
            assert_eq!(app.panels.panel, opened);
        }
    }
    app.workspace.wait_resources();
}

#[test]
fn r25_find_results_and_replacement_messages_keep_their_arguments() {
    let data = tempfile::tempdir().unwrap();
    let mut app = fixture(Some(data.path()));
    app.run_named(
        None,
        "show_panel",
        &json!({"panel":"replace", "pattern":"text", "replace_pattern":"text text"}),
    );
    app.run_named(None, "replace_all", &json!({}));
    assert_eq!(app.panels.message, "Replaced 1 occurrence");
    r25_language(&mut app, "zh-Hans");
    assert_eq!(app.panels.message, "已替换 1 处");
    r25_language(&mut app, "en");
    assert_eq!(app.panels.message, "Replaced 1 occurrence");

    app.run_named(
        None,
        "show_panel",
        &json!({"panel":"find_in_files", "pattern":"text", "use_buffer":false}),
    );
    app.panels.where_input.set("<current file>");
    // Give the current-file source a real file name without writing editor contents.
    let path = data.path().join("source.txt");
    fs::write(&path, "text text\n").unwrap();
    app.workspace
        .open(crate::cli::FileLocation {
            path,
            line: 1,
            column: 1,
        })
        .unwrap();
    app.run_named(None, "find_in_files_start", &json!({}));
    let deadline = Instant::now() + Duration::from_secs(10);
    while app.panels.message != "2 matches in 1 file" {
        let viewport = app.viewport();
        app.panels.poll(&mut app.workspace, viewport);
        assert!(
            Instant::now() < deadline,
            "file search: {}",
            app.panels.message
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    r25_paint(&mut app, &["Find Results · 2 matches in 1 file"]);
    r25_language(&mut app, "zh-Hans");
    r25_paint(&mut app, &["查找结果 · 在 1 个文件中找到 2 个匹配项"]);
    r25_language(&mut app, "en");
    r25_paint(&mut app, &["Find Results · 2 matches in 1 file"]);
    app.workspace.wait_resources();
}

#[test]
fn r25_sidebar_status_and_window_titles_follow_the_live_language() {
    let data = tempfile::tempdir().unwrap();
    let mut app = fixture(Some(data.path()));
    app.panels.side_visible = true;
    app.panels.open_files_visible = true;
    let e = app.workspace.editor_mut();
    e.settings
        .set(Layer::View, "set_unsaved_view_name", json!(false));
    e.settings
        .set(Layer::View, "translate_tabs_to_spaces", json!(true));
    e.set_selection(vel_buffer::Selection::from_regions([
        vel_buffer::Region::new(0, 6),
    ]));
    app.workspace.editor_mut().sync();
    for scale in [1., 2.] {
        app.workspace
            .editor_mut()
            .settings
            .set(Layer::View, "ui_scale", json!(scale));
        for language in ["en", "zh-Hans", "en"] {
            r25_language(&mut app, language);
            let (headings, tab, count) = if language == "en" {
                (
                    ["OPEN FILES", "FOLDERS", "Spaces: 4"],
                    "Untitled",
                    "6 characters selected",
                )
            } else {
                (
                    ["打开的文件", "文件夹", "空格：4"],
                    "未命名",
                    "已选择 6 个字符",
                )
            };
            r25_paint(&mut app, &headings);
            assert_eq!(app.workspace.window_title(), tab);
            assert_eq!(
                app.workspace.tabs[app.workspace.active].ui_label(&app.workspace.ui),
                tab
            );
            let position = crate::panels::status_position(&app.workspace);
            assert!(position.contains(count), "{position}");
            assert!(position.ends_with("UTF-8 · Unix"), "{position}");
            r25_paint(&mut app, &[&position]);
        }
    }
    app.workspace.wait_resources();
}

#[test]
fn r25_picker_and_input_labels_share_the_accessible_text() {
    let data = tempfile::tempdir().unwrap();
    let mut app = fixture(Some(data.path()));
    app.run_named(None, "goto_line", &json!({}));
    r25_paint(&mut app, &["Enter a line number"]);
    r25_language(&mut app, "zh-Hans");
    r25_paint(&mut app, &["输入行号"]);
    assert!(
        app.accessible_tree()
            .nodes
            .iter()
            .any(|node| node.label == "输入行号")
    );
    r25_language(&mut app, "en");
    r25_paint(&mut app, &["Enter a line number"]);
    app.panels
        .show_input_caption(
            &mut app.workspace,
            "show_input_panel",
            &json!({"initial_text":"Save"}),
            UiText::tr("New name for this entry"),
            0..0,
        )
        .unwrap();
    for (language, label) in [
        ("en", "New name for this entry"),
        ("zh-Hans", "输入新名称"),
        ("en", "New name for this entry"),
    ] {
        r25_language(&mut app, language);
        r25_paint(&mut app, &[label, "Save"]);
        let ax = app.accessible_tree();
        let node = ax
            .nodes
            .iter()
            .find(|node| node.label == label && node.text.is_some())
            .unwrap();
        assert_eq!(node.text.as_ref().unwrap().text.to_string(), "Save");
    }
    app.workspace.wait_resources();
}

#[test]
fn r25_open_appearance_and_symbol_rows_preserve_their_identity() {
    let data = tempfile::tempdir().unwrap();
    let mut app = fixture(Some(data.path()));
    app.run_named(None, "select_color_scheme", &json!({}));
    assert_eq!(app.panels.overlay, Some(crate::panels::Overlay::Appearance));
    let selected = app.panels.quick.current().unwrap();
    for (language, label) in [
        ("en", "Automatic (follow system appearance)"),
        ("zh-Hans", "自动（跟随系统外观）"),
        ("en", "Automatic (follow system appearance)"),
    ] {
        r25_language(&mut app, language);
        app.workspace.wait_resources();
        let viewport = app.viewport();
        app.panels.poll(&mut app.workspace, viewport);
        assert_eq!(app.panels.overlay, Some(crate::panels::Overlay::Appearance));
        assert_eq!(app.panels.quick.current(), Some(selected));
        assert!(
            app.panels
                .quick
                .items
                .iter()
                .any(|item| item.caption == label)
        );
    }
    app.panels.cancel_overlay(&mut app.workspace);
    let path = data.path().join("symbol.rs");
    fs::write(&path, "\nfn task() {}\n").unwrap();
    app.workspace
        .open(crate::cli::FileLocation {
            path,
            line: 1,
            column: 1,
        })
        .unwrap();
    app.run_named(
        None,
        "show_overlay",
        &json!({"overlay":"goto","text":"@task"}),
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    while !app
        .panels
        .quick
        .items
        .iter()
        .any(|item| item.caption.contains("task"))
    {
        app.poll_workspace_model();
        let viewport = app.viewport();
        app.panels.poll(&mut app.workspace, viewport);
        assert!(Instant::now() < deadline, "symbol picker did not publish");
        std::thread::sleep(Duration::from_millis(2));
    }
    let selected = app.panels.quick.current();
    for (language, annotation) in [("en", "Line 2"), ("zh-Hans", "第 2 行"), ("en", "Line 2")] {
        r25_language(&mut app, language);
        assert!(
            app.panels
                .quick
                .items
                .iter()
                .any(|item| item.caption.contains("task") && item.annotation == annotation)
        );
        assert_eq!(app.panels.quick.current(), selected);
    }
    app.workspace.wait_resources();
}

#[test]
fn r25_native_alert_buttons_settings_and_accessible_announcements() {
    let data = tempfile::tempdir().unwrap();
    let mut app = fixture(Some(data.path()));
    app.run_named(
        None,
        "edit_settings",
        &json!({"base_file":"Packages/Default/Preferences.sublime-settings"}),
    );
    let mut settings = app.opened_settings.pop().unwrap();
    let mut dialog = app.unsaved_changes_dialog(app.workspace.active);
    app.panels.set_message(UiText::tr("No matches"));
    for (language, buttons, title, status, about) in [
        (
            "en",
            ["Save", "Don't Save", "Cancel"],
            "Settings — Velocity",
            "No matches",
            "About Velocity",
        ),
        (
            "zh-Hans",
            ["保存", "不保存", "取消"],
            "设置 — Velocity",
            "无匹配项",
            "关于 Velocity",
        ),
        (
            "en",
            ["Save", "Don't Save", "Cancel"],
            "Settings — Velocity",
            "No matches",
            "About Velocity",
        ),
    ] {
        r25_language(&mut app, language);
        wait_language(&mut settings, language);
        dialog.localize(&app.workspace.translations);
        assert_eq!(
            dialog
                .buttons
                .iter()
                .map(UiText::as_str)
                .collect::<Vec<_>>(),
            buttons
        );
        let mut about_model = app.about_dialog();
        about_model.localize(&app.workspace.translations);
        assert_eq!(about_model.title, about);
        assert_eq!(settings.workspace.window_title(), title);
        let _ = app.scene();
        assert!(
            app.accessible_tree()
                .nodes
                .iter()
                .any(|node| node.announcement.as_deref() == Some(status))
        );
    }
    app.workspace.wait_resources();
}

#[test]
fn r25_build_headers_relabel_in_the_output_view_without_touching_tool_text() {
    let data = tempfile::tempdir().unwrap();
    let mut app = fixture(Some(data.path()));
    app.run_named(None, "exec", &json!({"cmd":["/bin/echo","[Cancelled]"]}));
    let deadline = Instant::now() + Duration::from_secs(10);
    while app.builds.task.is_some() {
        app.builds.poll(&mut app.panels);
        assert!(Instant::now() < deadline, "build did not finish");
        std::thread::sleep(Duration::from_millis(2));
    }
    let original = app.panels.output.clone();
    assert!(original.starts_with("[Cancelled]\n") && original.contains("[Finished in "));
    r25_language(&mut app, "zh-Hans");
    let viewport = app.viewport();
    app.panels.poll(&mut app.workspace, viewport);
    r25_paint(&mut app, &["输出"]);
    assert!(
        app.panels.output.starts_with("[Cancelled]\n")
            && app.panels.output.contains("[已完成，用时 ")
    );
    r25_language(&mut app, "en");
    assert_eq!(app.panels.output, original);
    app.workspace.wait_resources();
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
fn copy_as_html_dispatches_a_single_rich_item_from_menu_and_palette() {
    use std::cell::RefCell;
    use vel_buffer::{Region, Selection};
    #[derive(Default)]
    struct Clipboard(RefCell<Vec<(String, String)>>);
    impl RichClipboard for Clipboard {
        fn set_clipboard_html(&self, html: &str, plain_text: &str) -> bool {
            self.0.borrow_mut().push((html.into(), plain_text.into()));
            true
        }
    }
    let clipboard = Clipboard::default();
    let data = tempfile::tempdir().unwrap();
    let mut app = fixture(Some(data.path()));
    app.workspace.set_file_type("Rust").unwrap();
    let source = "let 我 = 1;\n\t// 猫 <&>\nlast\n";
    let editor = app.workspace.editor_mut();
    editor.run("select_all", &json!({})).unwrap();
    editor.run("insert", &json!({"characters":source})).unwrap();
    editor.set_selection(Selection::from_regions([
        Region::new(3, 0),
        Region::new(source.find("last").unwrap(), source.find('\t').unwrap()),
    ]));
    app.workspace.tabs[app.workspace.active].format.line_ending = vel_fileio::LineEnding::CrLf;
    r25_language(&mut app, "zh-Hans");
    let model = app.application_menu();
    let menu = rows(&model)
        .find(|r| r.command.as_deref() == Some("copy_as_html"))
        .unwrap();
    let command = menu.command.as_deref().unwrap();
    assert_eq!(menu.caption, "拷贝为 HTML");
    let args = serde_json::from_str(&menu.args).unwrap();
    assert!(app.command_enabled(command, &menu.args));
    app.dispatch_named(None, Some(&clipboard), command, &args, 0)
        .unwrap();
    let context = &app.workspace.context_menus["Context.sublime-menu"];
    assert!(
        context
            .iter()
            .any(|row| row.command.as_deref() == Some("copy_as_html"))
    );
    {
        let results = clipboard.0.borrow();
        let [(html, plain)] = results.as_slice() else {
            panic!("{results:?}")
        };
        assert_eq!(plain, "let\r\n\t// 猫 <&>\r\n");
        assert!(html.contains("<span style=\"color:"));
        assert!(html.contains("// 猫 &lt;&amp;&gt;"));
        assert!(!html.contains('\t') && !html.contains("last"));
        let active = app.workspace.editor().language.borrow();
        let color = active.as_ref().unwrap().scheme.default_style().background;
        assert!(html.contains(&format!(
            "background-color:#{:02x}{:02x}{:02x}",
            color.r, color.g, color.b
        )));
    }
    app.workspace.editor_mut().set_caret(0, false);
    open_palette(&mut app, "copy as html");
    let _ = palette_row(&mut app, "编辑：拷贝为 HTML");
    let viewport = app.viewport();
    let (_, inv) = app
        .panels
        .key(&mut app.workspace, &Key::parse("enter").unwrap(), viewport)
        .unwrap();
    let inv = inv.unwrap();
    assert_eq!(inv.command, "copy_as_html");
    app.dispatch_named(None, Some(&clipboard), &inv.command, &inv.args, 0)
        .unwrap();
    let results = clipboard.0.borrow();
    assert_eq!(results.len(), 2);
    assert_eq!(results[1].1, source.replace('\n', "\r\n"));
    assert!(results[1].0.contains('我') && results[1].0.contains("last"));
}

#[test]
fn copy_as_html_rejects_a_ten_megabyte_document_without_changing_the_clipboard() {
    struct NoWrite;
    impl RichClipboard for NoWrite {
        fn set_clipboard_html(&self, _: &str, _: &str) -> bool {
            panic!("large export must not touch the pasteboard")
        }
    }
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("large.txt");
    fs::write(&path, vec![b'x'; 10 * 1024 * 1024]).unwrap();
    let mut app = fixture(None);
    app.workspace
        .open(crate::cli::FileLocation {
            path,
            line: 1,
            column: 1,
        })
        .unwrap();
    assert_eq!(
        app.workspace.editor().buffer.text().len_bytes(),
        10 * 1024 * 1024
    );
    app.workspace.editor_mut().clipboard = "keep me".into();
    assert_eq!(
        app.dispatch_named(None, Some(&NoWrite), "copy_as_html", &json!({}), 0)
            .unwrap_err(),
        "Selection is too large to copy as HTML"
    );
    assert_eq!(app.workspace.editor().clipboard, "keep me");
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
    assert_eq!(app.workspace.ui_language_setting(), "en");
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
fn user_catalog_reloads_menus_palette_panels_status_and_accessibility() {
    fn language_rows(app: &mut App) -> Vec<(String, String, bool)> {
        let menu = app.application_menu();
        role(&role(&menu, "preferences").children, "language")
            .children
            .iter()
            .map(|row| {
                let tag = serde_json::from_str::<Value>(&row.args).unwrap()["language"]
                    .as_str()
                    .unwrap()
                    .to_owned();
                (
                    tag,
                    row.caption.clone(),
                    app.command_checked("set_ui_language", &row.args),
                )
            })
            .collect()
    }
    fn wait_for_catalog(app: &mut App, available: bool) {
        let deadline = Instant::now() + Duration::from_secs(15);
        while app.workspace.translations.catalog("x-test").is_some() != available {
            assert!(Instant::now() < deadline, "catalog reload did not publish");
            app.poll_workspace_model();
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    let data = tempfile::tempdir().unwrap();
    let mut app = fixture(Some(data.path()));
    assert!(
        !language_rows(&mut app)
            .iter()
            .any(|(tag, _, _)| tag == "x-test")
    );
    let localization = data.path().join("Packages/User/Localization");
    fs::create_dir_all(&localization).unwrap();
    let file = localization.join("x-test.sublime-localization");
    fs::write(
        &file,
        r#"{
        "language_name": "[Pséûdô]",
        "english_name": "Pseudo",
        "strings": {
            "File": "[Fïlê]",
            "Save": "[Ŝåvé]",
            "File: Save": "[Fïlê: Ŝåvé]",
            "Language": "[Långûågê]",
            "Follow System": "[Föllôw]",
            "Preferences: Language: {language}": "[Långûågê: {language}]",
            "Find": "[Fïnd]",
            "Regular expression": "[Řêgêx]",
            "Ln {line}, Col {column}": "[Lïné {line}, Çôl {column}]",
            "Unsupported UI language; using English": "[Ûnsûppôrtêd]"
        }
    }"#,
    )
    .unwrap();
    wait_for_catalog(&mut app, true);
    assert!(language_rows(&mut app).contains(&("x-test".into(), "[Pséûdô]".into(), false)));
    assert_eq!(
        app.workspace
            .translations
            .resolve_preferred(["x-test", "en"])
            .as_str(),
        "x-test"
    );
    open_palette(&mut app, "language pseudo");
    palette_row(&mut app, "Preferences: Language: Pseudo");
    app.run_named(None, "hide_overlay", &json!({}));
    app.run_named(None, "show_panel", &json!({"panel":"find"}));
    r25_paint(&mut app, &["Find", "Ln 1, Col 1 · UTF-8 · Unix"]);
    app.run_named(None, "set_ui_language", &json!({"language":"x-test"}));
    assert!(language_rows(&mut app).contains(&("x-test".into(), "[Pséûdô]".into(), true)));
    assert!(language_rows(&mut app).contains(&("auto".into(), "[Föllôw]".into(), false)));
    let menu = app.application_menu();
    assert_eq!(role(&menu, "file").caption, "[Fïlê]");
    assert!(
        rows(&menu).any(|row| row.command.as_deref() == Some("save") && row.caption == "[Ŝåvé]")
    );
    assert_eq!(
        role(&menu, "edit").caption,
        "Edit",
        "a missing key falls back"
    );
    assert_eq!(app.panels.panel, Some(crate::panels::Panel::Find));
    r25_paint(&mut app, &["[Fïnd]", "[Lïné 1, Çôl 1] · UTF-8 · Unix"]);
    assert!(
        app.accessible_tree()
            .nodes
            .iter()
            .any(|node| node.label == "[Řêgêx]")
    );
    open_palette(&mut app, "language pseudo");
    palette_row(&mut app, "[Långûågê: [Pséûdô]]");
    open_palette(&mut app, "save");
    palette_row(&mut app, "[Fïlê: Ŝåvé]");

    fs::remove_file(file).unwrap();
    wait_for_catalog(&mut app, false);
    assert_eq!(app.panels.message, "[Ûnsûppôrtêd]");
    assert!(app.workspace.translations.language().is_english());
    assert_eq!(app.workspace.ui_language_setting(), "x-test");
    let choices = language_rows(&mut app);
    assert!(!choices.iter().any(|(tag, _, _)| tag == "x-test"));
    assert!(choices.contains(&("en".into(), "English".into(), true)));
    assert!(
        !app.workspace
            .palette
            .iter()
            .any(|item| item.invocation.args["language"] == "x-test")
    );
    palette_row(&mut app, "File: Save");
    app.workspace.wait_resources();
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
fn shipped_catalogs_have_all_static_ui_captions() {
    for (tag, catalog) in crate::ui_language::shipped_catalogs() {
        check_static_ui_captions(&tag, &catalog);
    }
}

fn check_static_ui_captions(tag: &vel_i18n::Language, catalog: &vel_i18n::Catalog) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/Packages/Default");
    let mut captions = BTreeSet::from(["English".to_owned()]);
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
    app.workspace
        .recent_folders
        .push("/private/tmp/l10n-fixture".into());
    app.workspace.refresh_file_menus();
    let model = app.application_menu();
    captions.extend(
        rows(&model)
            .filter(|row| {
                row.caption != "-"
                    && !row.caption.bytes().all(|b| b.is_ascii_digit())
                    && !matches!(
                        row.command.as_deref(),
                        Some("open_recent_folder" | "set_file_type")
                    )
                    && (row.command.as_deref() != Some("set_build_system")
                        || row.caption == "Automatic")
                    && !row.id.starts_with("syntax_")
            })
            .filter(|row| catalog.strings.contains_key(&row.caption))
            .map(|row| row.caption.clone()),
    );

    let mut settings = app.workspace.settings.clone();
    // Menu-derived palette captions reuse individual group names, including
    // the group before a numeric argument (e.g. View: Layout: Columns: 2).
    for item in &app.workspace.palette {
        let caption = &item.caption;
        if !catalog.strings.contains_key(caption) && caption.starts_with("View: Layout: ") {
            captions.extend(
                caption
                    .split(": ")
                    .filter(|part| catalog.strings.contains_key(*part))
                    .map(str::to_owned),
            );
        }
    }
    settings.set(
        Layer::User,
        "package_index_url",
        json!("https://example.invalid"),
    );
    let mut generated = Vec::new();
    crate::release::extend_commands(&mut generated, &mut [], &settings);
    captions.extend(generated.into_iter().map(|item| item.caption));
    let rust_keys: &[&str] = include!(concat!(env!("OUT_DIR"), "/ui_keys.rs"));
    captions.extend(rust_keys.iter().map(|key| (*key).to_owned()));
    for source in crate::settings_ui::source_text() {
        assert!(
            catalog
                .strings
                .get(source)
                .is_some_and(|text| !text.trim().is_empty()),
            "{} missing setting copy: {source}",
            tag.as_str()
        );
        captions.insert(source.to_owned());
    }
    captions.extend(
        vel_fileio::ENCODINGS
            .iter()
            .map(|(name, _)| (*name).to_owned()),
    );
    let missing: Vec<_> = captions
        .iter()
        .filter(|key| !catalog.strings.contains_key(*key))
        .collect();
    assert!(missing.is_empty(), "{} missing: {missing:?}", tag.as_str());
    let orphans: BTreeSet<_> = catalog
        .strings
        .keys()
        .filter(|key| !captions.contains(*key))
        .collect();
    assert!(
        orphans.is_empty(),
        "{} orphan catalog keys: {orphans:#?}",
        tag.as_str()
    );
}

#[test]
fn shipped_catalogs_cover_the_assembled_palette() {
    let mut app = fixture(None);
    let before = app.workspace.palette.clone();
    for (tag, _) in crate::ui_language::shipped_catalogs() {
        app.workspace.translations.select(tag.clone());
        let missing: Vec<_> = app
            .workspace
            .palette
            .iter()
            .filter(|item| app.workspace.palette_caption(item).is_none())
            .map(|item| &item.caption)
            .collect();
        assert!(
            missing.is_empty(),
            "{} untranslated assembled palette: {missing:?}",
            tag.as_str()
        );
        assert_eq!(app.workspace.palette, before);
    }
}

#[test]
fn chinese_catalog_retains_group_separators() {
    // There are currently no exceptions. Add a named, explained entry here if
    // an English colon is punctuation rather than a UI group separator.
    const ALLOW_UNGROUPED: &[&str] = &[];
    let catalog = vel_i18n::Catalog::load(include_str!(
        "../resources/Packages/Default/Localization/zh-Hans.sublime-localization"
    ))
    .unwrap();
    let missing: BTreeSet<_> = catalog
        .strings
        .iter()
        .filter(|(source, translated)| {
            source.contains(": ")
                && !translated.contains('：')
                && !ALLOW_UNGROUPED.contains(&source.as_str())
        })
        .collect();
    assert!(
        missing.is_empty(),
        "Missing Chinese group separators: {missing:#?}"
    );
    assert_eq!(catalog.strings["File: Save"], "文件：保存");
    assert_eq!(catalog.strings["File: Save All"], "文件：全部保存");
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
        .select(vel_i18n::Language::parse("zh-Hans").unwrap());
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
        .select(vel_i18n::Language::parse("zh-Hans").unwrap());
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
                ))
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
    assert!(rows(&chinese).any(|row| row.caption == "宏：Add Line" && row.args.contains("Add Line.sublime-macro")));
}

#[test]
fn tools_macro_menu_localizes_prefix_but_not_names() {
    let data = tempfile::tempdir().unwrap();
    let user = data.path().join("Packages/User");
    fs::create_dir_all(&user).unwrap();
    // A name that is also a catalog key must remain resource data.
    fs::write(user.join("Save.sublime-macro"), "[]").unwrap();
    let mut app = fixture(Some(data.path()));
    let en = app.application_menu();
    app.workspace
        .translations
        .select(vel_i18n::Language::parse("zh-Hans").unwrap());
    let zh = app.application_menu();
    fn macros(menu: &[MenuEntry]) -> &[MenuEntry] {
        &role(&role(menu, "tools").children, "macros").children
    }
    let (en, zh) = (macros(&en), macros(&zh));
    assert_eq!(en.len(), zh.len());
    for (en, zh) in en.iter().zip(zh) {
        let name = en.caption.strip_prefix("Macro: ").unwrap();
        assert_eq!(zh.caption.strip_prefix("宏："), Some(name));
        assert_eq!(zh.command.as_deref(), Some("run_macro_file"));
        assert_eq!(zh.args, en.args);
    }
    assert!(zh.iter().any(|row| row.caption == "宏：User/Save"));
    assert!(zh.iter().any(|row| row.caption == "宏：Add Line"));
}

#[test]
fn command_icon_does_not_change_when_a_different_caption_alias_wins() {
    let mut app = fixture(None);
    open_palette(&mut app, "toggle tabs");
    let en = palette_row(&mut app, "View: Toggle Tabs").icon;
    assert_eq!(en, vel_ui::ItemIcon::View);
    app.workspace
        .translations
        .select(vel_i18n::Language::parse("zh-Hans").unwrap());
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
