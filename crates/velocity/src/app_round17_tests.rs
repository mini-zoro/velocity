use super::*;
use vel_buffer::{Region, Selection};
use vel_config::keymap::{Key, Keymap, Resolution};
use vel_text::Text;

fn app(text: &str, profile: Option<&Path>) -> App {
    let options = Options::default();
    let mut app = App::new(
        options.clone(),
        Clock::new(Instant::now(), std::time::SystemTime::now()),
        Reporter::for_options(&options, Instant::now()).unwrap(),
        Workspace::new(
            Text::from(text),
            profile.map(Path::to_path_buf),
            None,
            false,
        )
        .unwrap(),
        Arc::new(AtomicBool::new(false)),
        std::rc::Rc::new(std::cell::RefCell::new(crate::session::Sessions::new(
            profile,
        ))),
    );
    app.panels.side_visible = false;
    app.sync_active_view();
    layout(&mut app);
    app
}
fn layout(app: &mut App) {
    app.content = app.panels.layout(app.window_size);
    app.view.size = Size {
        width: app.content.width,
        height: app.content.height,
    };
    app.view.scene(app.workspace.editor());
}
fn named(app: &mut App, name: &str, args: Value) {
    app.run_named(None, name, &args);
}
fn key(app: &mut App, text: &str) {
    let viewport = app.viewport();
    assert!(
        app.dispatch_key_binding(None, &Key::parse(text).unwrap(), Instant::now(), viewport),
        "{text}"
    );
}
fn point(app: &App, row: usize, column: usize) -> Point {
    let at = app.workspace.editor().text.line_to_byte(row) + column;
    let rect = app.view.rect_at(app.workspace.editor(), at);
    Point {
        x: app.content.x + rect.x + 1.,
        y: app.content.y + rect.y + app.view.line_height / 2.,
    }
}
fn mouse(app: &mut App, position: Point, kind: MouseKind, count: usize, modifiers: u64) {
    app.handle_mouse_event(
        None,
        MouseEvent {
            button: 0,
            kind,
            position,
            click_count: count,
            modifiers: Modifiers(modifiers),
            timestamp: 0.,
        },
    );
}
fn fixture_profile(mousemap: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("Packages/User")).unwrap();
    std::fs::create_dir_all(dir.path().join("Installed Packages")).unwrap();
    std::fs::write(
        dir.path().join("Installed Packages/Broken.sublime-package"),
        b"broken archive",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("Packages/User/Mouse.sublime-mousemap"),
        mousemap,
    )
    .unwrap();
    dir
}

#[test]
fn real_resource_loader_isolates_bad_syntaxes_schemes_and_archives() {
    let profile = fixture_profile("[]");
    let user = profile.path().join("Packages/User");
    std::fs::write(user.join("Healthy.sublime-syntax"),"name: Healthy\nscope: source.healthy\ncontexts: {main: [{match: Z, scope: keyword.good}]}\n").unwrap();
    std::fs::write(
        user.join("Bad.sublime-syntax"),
        "scope: source.bad\ncontexts: {main: [{match: x, push: [[]]}]}\n",
    )
    .unwrap();
    std::fs::write(user.join("Bad.sublime-color-scheme"), "{").unwrap();
    std::fs::write(user.join("Legacy.hidden-tmTheme"),r##"<plist version="1.0"><dict><key>settings</key><array><dict><key>settings</key><dict><key>foreground</key><string>#123456</string><key>shadowWidth</key><string>6</string></dict></dict></array></dict></plist>"##).unwrap();
    std::fs::write(
        user.join("Preferences.sublime-settings"),
        r#"{"color_scheme":"Legacy.hidden-tmTheme"}"#,
    )
    .unwrap();
    for _ in 0..2 {
        // The second startup uses the actual cached user syntax index.
        let mut a = app("Z\n", Some(profile.path()));
        assert!(
            a.workspace
                .assets
                .syntaxes
                .find_syntax_by_name("Healthy")
                .is_some()
        );
        assert!(
            a.workspace
                .assets
                .syntaxes
                .warnings()
                .iter()
                .any(|w| w.contains("Bad.sublime-syntax"))
        );
        a.workspace
            .set_file_type("Packages/User/Healthy.sublime-syntax")
            .unwrap();
        let language = a.workspace.editor().language.borrow();
        let scheme = &language.as_ref().unwrap().scheme;
        assert!(scheme.hidden);
        assert_eq!(
            scheme.default_style().foreground,
            vel_color_scheme::Color::rgb(18, 52, 86)
        );
        assert_eq!(scheme.globals.number("shadow_width"), Some(6.));
    }
}

#[test]
fn resource_mousemap_reaches_real_press_drag_release_and_scroll_handlers() {
    let profile = fixture_profile(
        r#"[
      {"button":"button1","count":2,"modifiers":["shift"],"press_command":"drag_select","press_args":{"by":"words","extend":true}},
      {"button":"invalid","press_command":"insert"},
      {"button":1,"count":3,"modifiers":["shift"],"press_command":"drag_select","press_args":{"by":"lines","extend":true}},
      {"button":"button1","modifiers":["super"],"press_command":"drag_select","press_args":{"by":"words","additive":true}},
      {"button":"button1","modifiers":["super","shift"],"press_command":"drag_select","press_args":{"by":"words","subtractive":true}},
      {"button":"button16","command":"set_setting","args":{"setting":"extra_button","value":true}},
      {"button":"button2","press_command":"set_setting","press_args":{"setting":"secondary_button","value":true}},
      {"button":"button1","modifiers":["ctrl"],"press_command":"set_setting","press_args":{"setting":"control_button","value":true}},
      {"button":"scroll_up","command":"scroll_lines","args":{"amount":3}},
      {"button":"scroll_down","modifiers":["shift"],"command":"set_setting","args":{"setting":"wheel_binding","value":true}}
    ]"#,
    );
    let mut a = app("one two\nthree four\nlast\n", Some(profile.path()));
    assert_eq!(a.workspace.mousemap.len(), 9);
    a.workspace.editor_mut().set_caret(2, false);
    let target = point(&a, 1, 7);
    mouse(&mut a, target, MouseKind::Down, 2, 1 << 17);
    assert_eq!(a.workspace.editor().regions(), [Region::new(2, 18)]);
    let back = point(&a, 0, 1);
    mouse(&mut a, back, MouseKind::Drag, 2, 1 << 17);
    assert_eq!(a.workspace.editor().regions(), [Region::new(2, 0)]);
    mouse(&mut a, back, MouseKind::Up, 2, 1 << 17);
    assert!(a.drag_select.is_none());
    a.workspace.editor_mut().set_caret(2, false);
    mouse(&mut a, target, MouseKind::Down, 3, 1 << 17);
    assert_eq!(a.workspace.editor().regions(), [Region::new(2, 19)]);
    mouse(&mut a, target, MouseKind::Up, 3, 1 << 17);
    a.workspace
        .editor_mut()
        .set_selection(Selection::from_regions([Region::new(0, 3)]));
    mouse(&mut a, target, MouseKind::Down, 1, 1 << 20);
    assert_eq!(
        a.workspace.editor().regions(),
        [Region::new(0, 3), Region::new(14, 18)]
    );
    mouse(&mut a, target, MouseKind::Up, 1, 1 << 20);
    mouse(&mut a, back, MouseKind::Down, 1, (1 << 20) | (1 << 17));
    assert_eq!(a.workspace.editor().regions(), [Region::new(14, 18)]);
    mouse(&mut a, back, MouseKind::Up, 1, (1 << 20) | (1 << 17));
    a.handle_mouse_event(
        None,
        MouseEvent {
            button: 15,
            kind: MouseKind::Up,
            position: back,
            click_count: 1,
            modifiers: Modifiers(0),
            timestamp: 0.,
        },
    );
    assert!(a.workspace.editor().settings.bool("extra_button", false));
    for (button, modifiers, setting) in [
        (1, Modifiers(0), "secondary_button"),
        (0, Modifiers(1 << 18), "control_button"),
    ] {
        a.handle_mouse_event(
            None,
            MouseEvent {
                button,
                kind: MouseKind::Down,
                position: back,
                click_count: 1,
                modifiers,
                timestamp: 0.,
            },
        );
        assert!(a.workspace.editor().settings.bool(setting, false));
    }
    let mut long = app(&"line\n".repeat(500), Some(profile.path()));
    long.view.scroll_y = 100.;
    let event = ScrollEvent {
        position: point(&long, 8, 1),
        delta_x: 0.,
        delta_y: 1.,
        precise: false,
        phase: 0,
        momentum_phase: 0,
        modifiers: Modifiers(0),
        timestamp: 0.,
    };
    let before = long.view.scroll_y;
    long.handle_scroll_event(None, event);
    assert_eq!(long.view.scroll_y, before - 3. * long.view.line_height);
    long.handle_scroll_event(
        None,
        ScrollEvent {
            delta_y: -1.,
            modifiers: Modifiers(1 << 17),
            ..event
        },
    );
    assert!(
        long.workspace
            .editor()
            .settings
            .bool("wheel_binding", false)
    );
}

#[test]
fn mouse_bindings_respect_find_and_palette_hit_testing_and_document_focus() {
    let profile = fixture_profile(
        r#"[{"button":"button1","press_command":"insert","press_args":{"characters":"X"},"context":[{"key":"panel_has_focus","operand":false}]},{"button":"scroll_down","command":"insert","args":{"characters":"Y"}}]"#,
    );
    let mut a = app("one two\n", Some(profile.path()));
    named(&mut a, "show_panel", json!({"panel":"find","text":"two"}));
    layout(&mut a);
    let input = a.panels.query.bounds;
    let input_point = Point {
        x: input.x + 10.,
        y: input.y + 10.,
    };
    mouse(&mut a, input_point, MouseKind::Down, 1, 0);
    mouse(&mut a, input_point, MouseKind::Up, 1, 0);
    a.handle_scroll_event(
        None,
        ScrollEvent {
            position: input_point,
            delta_x: 0.,
            delta_y: -1.,
            precise: false,
            phase: 0,
            momentum_phase: 0,
            modifiers: Modifiers(0),
            timestamp: 0.,
        },
    );
    assert_eq!(a.workspace.editor().text.to_string(), "one two\n");
    // An ordinary unbound key refreshes keyboard contexts while Find owns focus.
    // The subsequent document click must not reuse that old focus context.
    let viewport = a.viewport();
    a.dispatch_key_binding(
        None,
        &Key::parse("ctrl+alt+shift+z").unwrap(),
        Instant::now(),
        viewport,
    );
    assert!(a.workspace.editor().ui_panel_focused);
    let target = point(&a, 0, 0);
    mouse(&mut a, target, MouseKind::Down, 1, 0);
    assert_eq!(a.workspace.editor().text.to_string(), "Xone two\n");
    named(&mut a, "show_overlay", json!({"overlay":"command_palette"}));
    layout(&mut a);
    let before = a.workspace.editor().text.to_string();
    mouse(&mut a, Point { x: 1., y: 1. }, MouseKind::Down, 1, 0);
    assert_eq!(a.workspace.editor().text.to_string(), before);
}

#[test]
fn indentation_commands_and_set_setting_use_named_dispatch_and_one_undo() {
    let original = " \tfoo\tbar\n\t  two\n    three\n";
    let mut a = app(original, None);
    named(
        &mut a,
        "set_setting",
        json!({"setting":"tab_size","value":4}),
    );
    a.workspace
        .editor_mut()
        .set_selection(Selection::from_regions([
            Region::new(5, 2),
            Region::caret(16),
        ]));
    named(&mut a, "expand_tabs", json!({"set_translate_tabs":true}));
    assert_eq!(
        a.workspace.editor().text.to_string(),
        "    foo\tbar\n      two\n    three\n"
    );
    assert!(
        a.workspace
            .editor()
            .settings
            .bool("translate_tabs_to_spaces", false)
    );
    named(&mut a, "undo", json!({}));
    assert_eq!(a.workspace.editor().text.to_string(), original);
    named(&mut a, "redo", json!({}));
    named(&mut a, "unexpand_tabs", json!({"set_translate_tabs":true}));
    assert_eq!(
        a.workspace.editor().text.to_string(),
        "\tfoo\tbar\n\t  two\n\tthree\n"
    );
    assert!(
        !a.workspace
            .editor()
            .settings
            .bool("translate_tabs_to_spaces", true)
    );
    named(&mut a, "undo", json!({}));
    assert_eq!(
        a.workspace.editor().text.to_string(),
        "    foo\tbar\n      two\n    three\n"
    );
    let version = a.workspace.editor().buffer.change_id();
    named(&mut a, "expand_tabs", json!({}));
    assert_eq!(a.workspace.editor().buffer.change_id(), version);
    named(
        &mut a,
        "set_setting",
        json!({"setting":"custom","value":["one",2,null]}),
    );
    assert_eq!(
        a.workspace.editor().settings.get("custom"),
        Some(&json!(["one", 2, null]))
    );
    let mut guess = app("root\n  first\n    nested\n  second\n", None);
    named(
        &mut guess,
        "set_setting",
        json!({"setting":"tab_size","value":8}),
    );
    named(&mut guess, "detect_indentation", json!({}));
    assert_eq!(guess.workspace.editor().preferences.tab_size, 2);
    assert!(!guess.workspace.editor().buffer.is_dirty());
    let mut big = app(&"\tcode\tvalue\n".repeat(20_000), None);
    named(&mut big, "expand_tabs", json!({}));
    assert!(
        big.workspace
            .editor()
            .text
            .line(19_999)
            .to_string()
            .starts_with("    code\t")
    );
    named(&mut big, "undo", json!({}));
    assert_eq!(
        big.workspace.editor().text.to_string(),
        "\tcode\tvalue\n".repeat(20_000)
    );
}

#[test]
fn round17_keymap_conflicts_and_real_scroll_key_dispatch() {
    // Compile the shipped file, then ask the real keymap to resolve each shortcut.
    let source = include_str!("../resources/Packages/Default/Velocity.sublime-keymap");
    let mut map = Keymap::default();
    map.append(source).unwrap();
    assert!(!map.is_empty());
    let mut a = app(&"line\n".repeat(500), None);
    for (keys, command, amount) in [
        ("ctrl+super+shift+f", "toggle_distraction_free", None),
        ("super+shift+alt+enter", "toggle_distraction_free", None),
        ("ctrl+alt+up", "scroll_lines", Some(1)),
        ("ctrl+alt+down", "scroll_lines", Some(-1)),
        ("ctrl+shift+up", "select_lines", None),
        ("ctrl+shift+down", "select_lines", None),
        ("super+=", "increase_font_size", None),
    ] {
        let Resolution::Command(invocation) = a
            .workspace
            .editor_mut()
            .resolve_key(&Key::parse(keys).unwrap())
        else {
            panic!("{keys}");
        };
        assert_eq!(invocation.command, command, "{keys}");
        if let Some(amount) = amount {
            assert_eq!(invocation.args["amount"], amount);
        }
    }
    a.view.scroll_y = 100.;
    key(&mut a, "ctrl+alt+up");
    assert_eq!(a.view.scroll_y, 100. - a.view.line_height);
    key(&mut a, "ctrl+alt+down");
    assert_eq!(a.view.scroll_y, 100.);
    named(&mut a, "show_panel", json!({"panel":"find","text":"line"}));
    key(&mut a, "ctrl+alt+down");
    assert_eq!(a.workspace.editor().text.to_string(), "line\n".repeat(500));
}

#[test]
fn bundled_diagnostics_never_enter_modal_queue_and_user_warnings_are_one_status() {
    let fresh = app("", None);
    assert!(
        fresh.workspace.messages.is_empty(),
        "{:?}",
        fresh.workspace.messages
    );
    assert!(fresh.workspace.resource_reports.is_empty());
    assert!(fresh.panels.message.is_empty(), "{}", fresh.panels.message);
    let profile = tempfile::tempdir().unwrap();
    let user = profile.path().join("Packages/User");
    std::fs::create_dir_all(&user).unwrap();
    std::fs::write(user.join("Warnings.sublime-syntax"),
        "name: Warnings\nscope: source.warnings\ncontexts: {main: [{include: absent_one}, {include: absent_two}]}\n").unwrap();
    for _ in 0..2 {
        let mut a = app("", Some(profile.path()));
        assert!(
            a.workspace.messages.is_empty(),
            "{:?}",
            a.workspace.messages
        );
        assert_eq!(
            a.panels.message,
            "2 syntax warnings (Packages/User/Warnings.sublime-syntax), see Velocity.log"
        );
        assert!(a.workspace.resource_reports.is_empty());
        a.panels.message.clear();
        a.workspace.request_resource_reload();
        a.workspace.wait_resources();
        a.publish_resource_reports();
        assert!(a.workspace.messages.is_empty());
        assert_eq!(
            a.panels.message,
            "2 syntax warnings (Packages/User/Warnings.sublime-syntax), see Velocity.log"
        );
    }
    std::fs::write(
        user.join("Warning.sublime-color-scheme"),
        r##"{"globals":{"foreground":"#123456","caret":"invalid!","invisibles":"invalid!"}}"##,
    )
    .unwrap();
    std::fs::write(user.join("Warning.sublime-mousemap"),
        r#"[{"button":"bad"},{"button":"also_bad"},{"button":"button16","command":"set_setting","args":{"setting":"valid_binding","value":true}}]"#).unwrap();
    std::fs::write(
        user.join("Velocity Dark.sublime-color-scheme"),
        r##"{"globals":{"line_highlight":"invalid!"}}"##,
    )
    .unwrap();
    let a = app("", Some(profile.path()));
    assert!(
        a.workspace.messages.is_empty(),
        "{:?}",
        a.workspace.messages
    );
    assert_eq!(a.workspace.mousemap.len(), 1);
    assert_eq!(
        a.panels.message,
        "2 syntax warnings (Packages/User/Warnings.sublime-syntax), see Velocity.log; 2 mousemap warnings (Packages/User/Warning.sublime-mousemap), see Velocity.log; 3 color-scheme warnings (Packages/User/Velocity Dark.sublime-color-scheme), see Velocity.log"
    );
    let log = std::fs::read_to_string(profile.path().join("Velocity.log")).unwrap();
    for path in [
        "Packages/User/Warnings.sublime-syntax",
        "Packages/User/Warning.sublime-mousemap",
        "Packages/User/Warning.sublime-color-scheme",
        "Packages/User/Velocity Dark.sublime-color-scheme",
    ] {
        assert!(log.contains(path), "{path}");
    }
}

#[test]
fn shell_cases_reindent_through_named_command_without_consuming_function_braces() {
    let cases = [
        (
            "f() {\ncase \"${1}\" in\nstart)\necho ok\n;;\nesac\n}\n",
            "f() {\n    case \"${1}\" in\n    start)\n        echo ok\n    ;;\n    esac\n}\n",
        ),
        (
            "case x in\nstart)\nf() {\necho ok\n}\n;;\nesac\n",
            "case x in\nstart)\n    f() {\n        echo ok\n    }\n;;\nesac\n",
        ),
        (
            "f() {\ncase \"${x:-y} ${#a[@]}\" in\nstart)\necho ok\n;;\nesac\n}\n",
            "f() {\n    case \"${x:-y} ${#a[@]}\" in\n    start)\n        echo ok\n    ;;\n    esac\n}\n",
        ),
    ];
    for (source, expected) in cases {
        let mut a = app(source, None);
        named(
            &mut a,
            "set_file_type",
            json!({"syntax":"Packages/ShellScript/Bash.sublime-syntax"}),
        );
        named(
            &mut a,
            "set_setting",
            json!({"setting":"translate_tabs_to_spaces","value":true}),
        );
        named(&mut a, "reindent", json!({}));
        let deadline = Instant::now() + Duration::from_secs(10);
        while a.workspace.editor().reindent_job.is_some() {
            a.workspace.poll();
            assert!(Instant::now() < deadline, "reindent did not finish");
            std::thread::yield_now();
        }
        assert_eq!(a.workspace.editor().text.to_string(), expected, "{source}");
        named(&mut a, "undo", json!({}));
        assert_eq!(a.workspace.editor().text.to_string(), source);
    }
}

#[test]
fn indentation_commands_keep_find_focus_and_skip_heredoc_content() {
    let mut a = app("\tcode\n", None);
    named(
        &mut a,
        "show_panel",
        json!({"panel":"find","text":"  query"}),
    );
    let query = a.panels.query.value().to_owned();
    named(&mut a, "expand_tabs", json!({}));
    assert_eq!(a.workspace.editor().text.to_string(), "    code\n");
    assert_eq!(a.panels.query.value(), query);
    assert!(a.panels.input().is_some(), "Find retains keyboard focus");
    named(&mut a, "unexpand_tabs", json!({}));
    assert_eq!(a.workspace.editor().text.to_string(), "\tcode\n");
    assert_eq!(a.panels.query.value(), query);
    assert!(a.panels.input().is_some(), "Find retains keyboard focus");
    named(
        &mut a,
        "set_setting",
        json!({"setting":"tab_size","value":8}),
    );
    named(&mut a, "detect_indentation", json!({}));
    assert_eq!(a.workspace.editor().preferences.tab_size, 8);
    assert_eq!(a.panels.query.value(), query);
    assert!(a.panels.input().is_some(), "Find retains keyboard focus");
    let source =
        "if true; then\n\tcat <<-EOF\n\tcontent\n\tEOF\n    cat <<'OTHER'\n    spaces\nOTHER\nfi\n";
    let mut a = app(source, None);
    named(
        &mut a,
        "set_file_type",
        json!({"syntax":"Packages/ShellScript/Bash.sublime-syntax"}),
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        a.workspace.poll();
        if a.workspace
            .editor()
            .language
            .borrow()
            .as_ref()
            .is_some_and(|l| l.scopes_ready())
        {
            break;
        }
        assert!(Instant::now() < deadline, "shell scopes did not arrive");
        std::thread::yield_now();
    }
    named(&mut a, "expand_tabs", json!({}));
    assert_eq!(
        a.workspace.editor().text.to_string(),
        source.replacen("\tcat", "    cat", 1)
    );
    // Publish scopes for the new version before converting in the other direction.
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        a.workspace.poll();
        if a.workspace
            .editor()
            .language
            .borrow()
            .as_ref()
            .is_some_and(|l| l.scopes_ready())
        {
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
    named(&mut a, "unexpand_tabs", json!({}));
    assert_eq!(
        a.workspace.editor().text.to_string(),
        source.replace("    cat", "\tcat")
    );
}

#[test]
fn tab_indented_load_and_guess_respect_live_user_and_syntax_width() {
    for local in [false, true] {
        let profile = tempfile::tempdir().unwrap();
        let user = profile.path().join("Packages/User");
        std::fs::create_dir_all(&user).unwrap();
        let prefs = user.join("Preferences.sublime-settings");
        std::fs::write(
            &prefs,
            r#"{"index_files":false,"tab_size":8,"translate_tabs_to_spaces":true}"#,
        )
        .unwrap();
        let settings = if local {
            let syntax = user.join("Go.sublime-settings");
            std::fs::write(&syntax, r#"{"tab_size":6}"#).unwrap();
            syntax
        } else {
            prefs
        };
        let file = profile.path().join("main.go");
        std::fs::write(
            &file,
            "package main\nfunc main() {\n\tcall()\n\tcall()\n}\n",
        )
        .unwrap();
        let mut a = app("", Some(profile.path()));
        a.workspace
            .open(crate::cli::FileLocation {
                path: file,
                line: 1,
                column: 1,
            })
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(20);
        while a.workspace.tabs[a.workspace.active].loading.is_some() {
            a.poll_workspace_model();
            assert!(Instant::now() < deadline);
            std::thread::park_timeout(Duration::from_millis(2));
        }
        assert_eq!(
            a.workspace.editor().preferences.tab_size,
            if local { 6 } else { 8 }
        );
        assert!(!a.workspace.editor().preferences.translate_tabs_to_spaces);
        named(&mut a, "detect_indentation", json!({}));
        std::fs::write(
            settings,
            r#"{"index_files":false,"tab_size":7,"translate_tabs_to_spaces":true}"#,
        )
        .unwrap();
        a.workspace.request_resource_reload();
        let deadline = Instant::now() + Duration::from_secs(20);
        while a.workspace.editor().preferences.tab_size != 7 {
            a.poll_workspace_model();
            assert!(Instant::now() < deadline, "tab width did not reload");
            std::thread::park_timeout(Duration::from_millis(2));
        }
        assert!(!a.workspace.editor().preferences.translate_tabs_to_spaces);
    }
}

#[test]
fn guessing_tabs_clears_only_the_width_owned_by_an_earlier_guess() {
    for (before, after, expected) in [(None, None, 8), (Some(6), None, 6), (None, Some(2), 2)] {
        let mut a = app("root\n  a\n    b\n  c\nroot\n", None);
        let e = a.workspace.editor_mut();
        e.settings
            .set(vel_config::settings::Layer::User, "tab_size", json!(8));
        e.sync();
        if let Some(width) = before {
            named(
                &mut a,
                "set_setting",
                json!({"setting":"tab_size","value":width}),
            );
        }
        named(&mut a, "detect_indentation", json!({}));
        assert_eq!(a.workspace.editor().preferences.tab_size, 2);
        if let Some(width) = after {
            // Even an explicit choice equal to the former guess takes ownership.
            named(
                &mut a,
                "set_setting",
                json!({"setting":"tab_size","value":width}),
            );
        }
        key(&mut a, "super+a");
        a.workspace.editor_mut().clipboard = "root\n\ta\n\tb\n".into();
        key(&mut a, "super+v");
        named(&mut a, "detect_indentation", json!({}));
        let e = a.workspace.editor();
        assert!(!e.preferences.translate_tabs_to_spaces);
        assert_eq!(e.preferences.tab_size, expected);
    }
}

#[test]
fn minimal_user_scheme_keeps_fold_ellipsis_visible_through_named_fold() {
    let profile = tempfile::tempdir().unwrap();
    let user = profile.path().join("Packages/User");
    std::fs::create_dir_all(&user).unwrap();
    std::fs::write(
        user.join("Minimal.sublime-color-scheme"),
        r##"{"globals":{"background":"#fff","foreground":"#123456"}}"##,
    )
    .unwrap();
    std::fs::write(
        user.join("Preferences.sublime-settings"),
        r#"{"color_scheme":"Minimal.sublime-color-scheme"}"#,
    )
    .unwrap();
    let mut a = app("visible hidden tail\n", Some(profile.path()));
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        a.workspace.poll();
        if a.workspace
            .editor()
            .language
            .borrow()
            .as_ref()
            .is_some_and(|l| l.ready())
        {
            break;
        }
        assert!(Instant::now() < deadline, "fold analysis did not arrive");
        std::thread::yield_now();
    }
    let id = a.workspace.editor().id;
    a.workspace
        .editor_mut()
        .buffer
        .set_selection(id, Selection::from_regions([Region::new(8, 14)]));
    named(&mut a, "fold", json!({}));
    // Exercise fold_marker ink rather than the selected-text foreground.
    named(&mut a, "move_to", json!({"to":"bof"}));
    let scene = a.view.scene(a.workspace.editor());
    let ellipsis = scene
        .layers
        .iter()
        .flat_map(|l| &l.text)
        .find(|run| run.text.contains('…'))
        .expect("folded ellipsis");
    assert_eq!(
        ellipsis.color,
        crate::language::rgba(vel_color_scheme::Color::rgb(18, 52, 86))
    );
    assert!(ellipsis.color[3] > 0.);
}

#[test]
fn legacy_scalar_syntax_files_survive_real_package_loader_and_named_selection() {
    let profile = fixture_profile("[]");
    let user = profile.path().join("Packages/User");
    for (name, contents) in [
        (
            "Quoted",
            "scope: source.quoted\ncontexts: {main: [{match: '(x)', captures: {'1': keyword.scalar}}]}",
        ),
        (
            "Empty",
            "scope:\ncontexts: {main: [{match: x, scope: keyword.scalar}]}",
        ),
        (
            "Numeric",
            "scope: source.numeric\nfile_extensions: [man, 1, 2, 3]\ncontexts: {main: [{match: x, scope: keyword.scalar}]}",
        ),
    ] {
        std::fs::write(
            user.join(format!("{name}.sublime-syntax")),
            format!("name: {name}\n{contents}"),
        )
        .unwrap();
    }
    for _ in 0..2 {
        let mut a = app("x\n", Some(profile.path()));
        for name in ["Quoted", "Empty", "Numeric"] {
            let path = format!("Packages/User/{name}.sublime-syntax");
            assert!(
                !a.workspace
                    .assets
                    .syntaxes
                    .warnings()
                    .iter()
                    .any(|w| w.contains(&path))
            );
            named(&mut a, "set_file_type", json!({"syntax":path}));
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                a.workspace.poll();
                let e = a.workspace.editor();
                if e.language
                    .borrow()
                    .as_ref()
                    .is_some_and(|l| l.scopes_ready())
                {
                    break;
                }
                assert!(Instant::now() < deadline, "{name} scopes did not arrive");
                std::thread::yield_now();
            }
            let e = a.workspace.editor();
            let language = e.language.borrow();
            let language = language.as_ref().unwrap();
            assert_eq!(language.name, name);
            assert!(
                language
                    .scopes(&e.text, 0)
                    .as_slice()
                    .iter()
                    .any(|s| s.build_string() == "keyword.scalar")
            );
        }
        assert_eq!(
            a.workspace
                .assets
                .syntaxes
                .find_syntax_by_extension("1")
                .unwrap()
                .name,
            "Numeric"
        );
    }
}

#[test]
fn vintage_noop_dot_target_reaches_real_key_dispatch_without_phantom_undo() {
    for (source, commands, expected) in [
        ("first\nb\nlast\n", "ddrb.", "b\nlast\n"),
        ("first\n123\nlast\n", "dd~.", "123\nlast\n"),
        ("first\nlower\nlast\n", "ddguu.", "lower\nlast\n"),
    ] {
        let mut a = app(source, None);
        named(
            &mut a,
            "set_setting",
            json!({"setting":"vintage_start_in_command_mode","value":true}),
        );
        named(
            &mut a,
            "set_setting",
            json!({"setting":"vintage_enabled","value":true}),
        );
        for c in commands.chars() {
            key(&mut a, &c.to_string());
        }
        assert_eq!(
            a.workspace.editor().text.to_string(),
            expected,
            "{commands}"
        );
        key(&mut a, "u");
        assert_eq!(a.workspace.editor().text.to_string(), source, "{commands}u");
    }
}

fn wait_scopes(a: &mut App) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        a.workspace.poll();
        if a.workspace
            .editor()
            .language
            .borrow()
            .as_ref()
            .is_some_and(|l| l.scopes_ready())
        {
            return;
        }
        assert!(Instant::now() < deadline, "scopes did not arrive");
        std::thread::yield_now();
    }
}

#[test]
fn tab_conversion_parses_cold_and_stale_scopes_before_changing_indentation() {
    for (command, prefix, converted) in [
        ("expand_tabs", "\t", "    "),
        ("unexpand_tabs", "    ", "\t"),
    ] {
        for stale in [false, true] {
            let source = format!(
                "# before\nif true; then\n{prefix}cat <<-EOF\n\t  \tkeep\n\tEOF\n{prefix}cat <<'OTHER'\n    spaces\nOTHER\nfi\n"
            );
            let mut a = app(&source, None);
            named(
                &mut a,
                "set_file_type",
                json!({"syntax":"Packages/ShellScript/Bash.sublime-syntax"}),
            );
            let extra = if stale {
                wait_scopes(&mut a);
                a.workspace.editor_mut().set_caret(0, false);
                named(&mut a, "insert", json!({"characters":"#"}));
                "#"
            } else {
                ""
            };
            assert!(
                !a.workspace
                    .editor()
                    .language
                    .borrow()
                    .as_ref()
                    .unwrap()
                    .scopes_ready()
            );
            named(&mut a, command, json!({}));
            let expected = extra.to_owned()
                + &source.replace(&format!("{prefix}cat"), &format!("{converted}cat"));
            assert_eq!(
                a.workspace.editor().text.to_string(),
                expected,
                "{command}, stale={stale}"
            );
            named(&mut a, "undo", json!({}));
            assert_eq!(
                a.workspace.editor().text.to_string(),
                extra.to_owned() + &source
            );
        }
    }
}

#[test]
fn tab_conversion_keeps_host_heredoc_whitespace_with_embedded_plain_text_fallback() {
    for (command, prefix, converted) in [
        ("expand_tabs", "\t", "    "),
        ("unexpand_tabs", "    ", "\t"),
    ] {
        let source = format!(
            "{prefix}value = <<~'HTML'\n{prefix}<div>\n{prefix}text\n{prefix}HTML\n{prefix}puts :after\n"
        );
        let mut a = app(&source, None);
        named(
            &mut a,
            "set_file_type",
            json!({"syntax":"Packages/Ruby/Ruby.sublime-syntax"}),
        );
        named(&mut a, command, json!({}));
        let expected = source
            .replace(&format!("{prefix}value"), &format!("{converted}value"))
            .replace(&format!("{prefix}puts"), &format!("{converted}puts"));
        assert_eq!(a.workspace.editor().text.to_string(), expected, "{command}");
    }
}

#[test]
fn reindent_keeps_literal_prefix_bytes_and_treats_shell_parameters_as_atomic() {
    let cases = [
        (
            "f() {\ncase x in\na)\ncat <<-EOF\n\t  \tkeep\n\tEOF\n;;\nesac\necho after\n}\n",
            "f() {\n    case x in\n    a)\n        cat <<-EOF\n\t  \tkeep\n\tEOF\n    ;;\n    esac\n    echo after\n}\n",
        ),
        (
            "f() {\ncase x in\na)\necho ${#a[@]}\n;;\nb)\necho ${x:-y}\n;;\nesac\n}\n",
            "f() {\n    case x in\n    a)\n        echo ${#a[@]}\n    ;;\n    b)\n        echo ${x:-y}\n    ;;\n    esac\n}\n",
        ),
        (
            "f() {\ncase $x in a) echo a;; esac\necho after\n}\n",
            "f() {\n    case $x in a) echo a;; esac\n    echo after\n}\n",
        ),
        (
            "f() {\ncase x in\na)\necho ${a[${i}]:-${other}}\n;;\nb)\necho b\n;;\nesac\n}\n",
            "f() {\n    case x in\n    a)\n        echo ${a[${i}]:-${other}}\n    ;;\n    b)\n        echo b\n    ;;\n    esac\n}\n",
        ),
    ];
    for syntax in [
        "Packages/ShellScript/Bash.sublime-syntax",
        "Packages/ShellScript/Zsh.sublime-syntax",
    ] {
        for (source, expected) in cases {
            let mut a = app(source, None);
            named(&mut a, "set_file_type", json!({"syntax":syntax}));
            named(
                &mut a,
                "set_setting",
                json!({"setting":"translate_tabs_to_spaces","value":true}),
            );
            named(&mut a, "reindent", json!({}));
            let deadline = Instant::now() + Duration::from_secs(10);
            while a.workspace.editor().reindent_job.is_some() {
                a.workspace.poll();
                assert!(Instant::now() < deadline);
                std::thread::yield_now();
            }
            assert_eq!(
                a.workspace.editor().text.to_string(),
                expected,
                "{syntax}: {source}"
            );
            named(&mut a, "undo", json!({}));
            assert_eq!(a.workspace.editor().text.to_string(), source);
        }
    }
}

#[test]
fn reindent_keeps_multiline_string_whitespace_bytes_through_named_command() {
    for (syntax, source, expected) in [
        (
            "Packages/Python/Python.sublime-syntax",
            "if True:\n\tvalue = \"\"\"\n \t  data\n\t\"\"\"\n\tpass\n",
            "if True:\n    value = \"\"\"\n \t  data\n\t\"\"\"\n    pass\n",
        ),
        (
            "Packages/C++/C++.sublime-syntax",
            "void f() {\nauto s = R\"tag(\n\t  \tliteral\n    spaces\n\t)tag\";\nnext();\n}\n",
            "void f() {\n    auto s = R\"tag(\n\t  \tliteral\n    spaces\n\t)tag\";\n    next();\n}\n",
        ),
        (
            "Packages/JavaScript/JavaScript.sublime-syntax",
            "`\n    literal ${x}\n`\nfunction f() {\nreturn 1;\n}\n",
            "`\n    literal ${x}\n`\nfunction f() {\n    return 1;\n}\n",
        ),
    ] {
        let mut a = app(source, None);
        named(&mut a, "set_file_type", json!({"syntax":syntax}));
        named(
            &mut a,
            "set_setting",
            json!({"setting":"translate_tabs_to_spaces","value":true}),
        );
        named(&mut a, "reindent", json!({}));
        let deadline = Instant::now() + Duration::from_secs(10);
        while a.workspace.editor().reindent_job.is_some() {
            a.workspace.poll();
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
        assert_eq!(a.workspace.editor().text.to_string(), expected, "{syntax}");
        named(&mut a, "undo", json!({}));
        assert_eq!(a.workspace.editor().text.to_string(), source);
    }
}

#[test]
fn empty_vintage_insert_and_replace_are_dot_targets_through_key_dispatch() {
    for insert in ["i", "shift+a", "shift+r"] {
        let source = "remove\nkeep\nlast\n";
        let mut a = app(source, None);
        named(
            &mut a,
            "set_setting",
            json!({"setting":"vintage_start_in_command_mode","value":true}),
        );
        named(
            &mut a,
            "set_setting",
            json!({"setting":"vintage_enabled","value":true}),
        );
        for command in ["d", "d", insert, "escape", "."] {
            key(&mut a, command);
        }
        assert_eq!(
            a.workspace.editor().text.to_string(),
            "keep\nlast\n",
            "{insert}"
        );
        key(&mut a, "u");
        assert_eq!(
            a.workspace.editor().text.to_string(),
            source,
            "{insert}: no phantom undo group"
        );
    }
}

#[test]
fn resource_warnings_persist_to_log_and_corrupt_packages_never_enter_modal_queue() {
    use std::io::Write;
    let profile = fixture_profile("[]");
    let user = profile.path().join("Packages/User");
    std::fs::write(user.join("Warnings.sublime-syntax"),
        "name: Warnings\nscope: source.warnings\ncontexts: {main: [{include: absent_one}, {include: absent_two}]}\n").unwrap();
    std::fs::write(
        user.join("Healthy.sublime-syntax"),
        "name: Healthy\nscope: source.healthy\ncontexts: {main: []}\n",
    )
    .unwrap();
    let loose = profile.path().join("Packages/Extra/Loose.sublime-syntax");
    std::fs::create_dir_all(loose.parent().unwrap()).unwrap();
    std::fs::write(
        &loose,
        "name: Loose\nscope: source.loose\ncontexts: {main: [{include: absent_loose}]}\n",
    )
    .unwrap();
    let mut archive = zip::ZipWriter::new(
        std::fs::File::create(
            profile
                .path()
                .join("Installed Packages/Installed.sublime-package"),
        )
        .unwrap(),
    );
    archive
        .start_file(
            "Installed.sublime-syntax",
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
    archive.write_all(b"name: Installed\nscope: source.installed\ncontexts: {main: [{include: absent_archive}]}\n").unwrap();
    archive.finish().unwrap();
    let log = profile.path().join("Velocity.log");
    for _ in 0..2 {
        let mut a = app("Z\n", Some(profile.path()));
        assert!(
            a.workspace.messages.is_empty(),
            "{:?}",
            a.workspace.messages
        );
        assert!(
            a.panels.message.contains("1 package warnings"),
            "{}",
            a.panels.message
        );
        assert!(a.panels.message.contains("Broken.sublime-package"));
        assert!(
            a.panels.message.contains("4 syntax warnings"),
            "{}",
            a.panels.message
        );
        assert!(
            a.panels
                .message
                .contains("Packages/Extra/Loose.sublime-syntax")
        );
        assert!(a.panels.message.contains("see Velocity.log"));
        let contents = std::fs::read_to_string(&log).unwrap();
        for detail in [
            "Packages/User/Warnings.sublime-syntax",
            "absent_one",
            "absent_two",
            "Packages/Extra/Loose.sublime-syntax",
            "absent_loose",
            "Packages/Installed/Installed.sublime-syntax",
            "absent_archive",
            "Broken.sublime-package",
        ] {
            assert!(contents.contains(detail), "{detail}");
        }
        named(
            &mut a,
            "set_file_type",
            json!({"syntax":"Packages/User/Healthy.sublime-syntax"}),
        );
        assert_eq!(
            a.workspace
                .editor()
                .language
                .borrow()
                .as_ref()
                .unwrap()
                .name,
            "Healthy"
        );
        let old_len = contents.len();
        a.workspace.request_resource_reload();
        a.workspace.wait_resources();
        a.publish_resource_reports();
        assert!(a.workspace.messages.is_empty());
        assert!(a.panels.message.contains("4 syntax warnings"));
        assert!(std::fs::read_to_string(&log).unwrap().len() > old_len);
    }
    // A user can open this ordinary UTF-8 file with the existing document loader.
    let mut w = Workspace::new(Text::new(), Some(profile.path().into()), None, false).unwrap();
    w.open(crate::cli::FileLocation {
        path: log,
        line: 1,
        column: 1,
    })
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while w.tabs[w.active].loading.is_some() {
        w.poll();
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
    assert!(w.editor().text.to_string().contains("absent_one"));
}

#[test]
fn syntax_warning_provenance_uses_resource_paths_even_when_display_names_collide() {
    let profile = tempfile::tempdir().unwrap();
    let user = profile.path().join("Packages/User");
    std::fs::create_dir_all(&user).unwrap();
    // A disabled bundled parent produces a warning from Rails, mentioning its name.
    // A healthy User syntax with that same name must not acquire that warning.
    std::fs::write(
        user.join("Preferences.sublime-settings"),
        r#"{"ignored_packages":["Ruby","Vintage"]}"#,
    )
    .unwrap();
    let file = user.join("Duplicate.sublime-syntax");
    std::fs::write(
        &file,
        "name: Ruby (Rails)\nscope: source.duplicate\ncontexts: {main: []}\n",
    )
    .unwrap();
    let a = app("", Some(profile.path()));
    assert!(
        a.workspace
            .assets
            .syntaxes
            .warnings()
            .iter()
            .any(|warning| warning
                .starts_with("Packages/Rails/Ruby (Rails).sublime-syntax: syntax 'Ruby (Rails)'"))
    );
    assert!(a.workspace.messages.is_empty());
    assert!(a.panels.message.is_empty(), "{}", a.panels.message);
    for contents in [
        "name: Ruby (Rails)\nscope: source.duplicate\ncontexts: {main: [{include: absent_user}]}\n",
        "name: Ruby (Rails)\nscope: source.duplicate\nextends: Packages/Absent/Parent.sublime-syntax\ncontexts: {main: []}\n",
    ] {
        std::fs::write(&file, contents).unwrap();
        for _ in 0..2 {
            let a = app("", Some(profile.path()));
            assert!(a.workspace.messages.is_empty());
            assert_eq!(
                a.panels.message,
                "1 syntax warnings (Packages/User/Duplicate.sublime-syntax), see Velocity.log"
            );
            let log = std::fs::read_to_string(profile.path().join("Velocity.log")).unwrap();
            assert!(log.contains("Packages/User/Duplicate.sublime-syntax"));
        }
    }
}
