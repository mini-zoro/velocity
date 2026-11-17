//! Round 18 Lane B: real resource, command, sidebar, build and frame paths.
use super::*;
use crate::cli::FileLocation;
use vel_text::Text;
fn app(text: &str, profile: Option<&Path>) -> App {
    let options = Options::default();
    let workspace = Workspace::new(
        Text::from(text),
        profile.map(Path::to_path_buf),
        None,
        false,
    )
    .unwrap();
    let mut app = App::new(
        options.clone(),
        Clock::new(Instant::now(), std::time::SystemTime::now()),
        Reporter::for_options(&options, Instant::now()).unwrap(),
        workspace,
        Arc::new(AtomicBool::new(false)),
        std::rc::Rc::new(std::cell::RefCell::new(crate::session::Sessions::new(
            profile,
        ))),
    );
    app.panels.side_visible = false;
    app.sync_active_view();
    app.content = app.panels.layout(app.window_size);
    app.view.size = Size {
        width: app.content.width,
        height: app.content.height,
    };
    app.view.scene(app.workspace.editor());
    app
}
fn named(a: &mut App, name: &str, args: Value) {
    a.run_named(None, name, &args);
}
fn set(a: &mut App, name: &str, value: Value) {
    named(a, "set_setting", json!({"setting":name,"value":value}));
}
fn key(a: &mut App, name: &str) -> bool {
    let viewport = a.viewport();
    a.dispatch_key_binding(
        None,
        &vel_config::keymap::Key::parse(name).unwrap(),
        Instant::now(),
        viewport,
    )
}
fn finish(a: &mut App) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while a.builds.task.is_some() {
        a.builds.poll(&mut a.panels);
        assert!(Instant::now() < deadline, "build timeout");
        std::thread::sleep(Duration::from_millis(2));
    }
}
fn mouse(a: &mut App, position: Point, clicks: usize) {
    for kind in [MouseKind::Down, MouseKind::Up] {
        a.handle_mouse_event(
            None,
            MouseEvent {
                position,
                kind,
                button: 0,
                click_count: clicks,
                modifiers: Modifiers(0),
                timestamp: 0.,
            },
        );
    }
}
#[test]
fn round18_command_reveal_table_preserves_parked_views_and_follows_real_changes() {
    let mut a = app(&"line text\n".repeat(6000), None);
    a.workspace
        .editor_mut()
        .keymap
        .append(r#"[{"keys":["ctrl+k","ctrl+x"],"command":"noop"}]"#)
        .unwrap();
    a.view.scroll_y = 5000. * a.view.line_height;
    let parked = a.view.scroll_y;
    assert!(key(&mut a, "ctrl+k"));
    assert_eq!(a.view.scroll_y, parked);
    let _ = key(&mut a, "escape");
    named(&mut a, "focus_side_bar", json!({}));
    assert!(key(&mut a, "down"));
    assert_eq!(a.view.scroll_y, parked);
    a.panels.side_focused = false;
    for (name, args) in [
        ("set_setting", json!({"setting":"tab_size","value":2})),
        ("toggle_setting", json!({"setting":"highlight_line"})),
        (
            "set_file_type",
            json!({"syntax":"Packages/Rust/Rust.sublime-syntax"}),
        ),
        ("show_overlay", json!({"overlay":"command_palette"})),
        ("toggle_minimap", json!({})),
        (
            "set_setting",
            json!({"setting":"theme","value":"Velocity Light.sublime-theme"}),
        ),
        ("copy", json!({})),
    ] {
        let _ = key(&mut a, "escape");
        a.workspace.editor_mut().set_caret(0, false);
        a.view.scroll_y = 5000. * a.view.line_height;
        let before = a.view.scroll_y;
        named(&mut a, name, args);
        assert_eq!(a.view.scroll_y, before, "{name}: {}", a.panels.message);
    }
    let _ = key(&mut a, "escape");
    set(&mut a, "scroll_speed", json!(0.));
    for (name, args) in [
        ("move", json!({"by":"characters","forward":true})),
        ("insert", json!({"characters":"x"})),
        ("undo", json!({})),
        ("redo", json!({})),
        ("show_at_center", json!({})),
    ] {
        a.workspace.editor_mut().set_caret(0, false);
        a.view.scroll_y = 5000. * a.view.line_height;
        named(&mut a, name, args);
        assert!(a.view.scroll_y < 1000., "{name}");
    }
    a.workspace.editor_mut().set_caret(0, false);
    a.view.scroll_y = 5000. * a.view.line_height;
    named(&mut a, "show_panel", json!({"panel":"find","text":"line"}));
    named(&mut a, "find_next", json!({}));
    assert!(a.view.scroll_y < 1000.);
    let _ = key(&mut a, "escape");
    named(&mut a, "goto_line", json!({}));
    a.panels.quick_input.set(":4000");
    let viewport = a.viewport();
    a.panels.edited(&mut a.workspace, viewport);
    assert_eq!(
        a.workspace
            .editor()
            .text
            .byte_to_line(a.workspace.editor().caret),
        3999
    );
    a.scene();
    assert!(a.view.scroll_y > 3000. * a.view.line_height);
}
#[test]
fn round18_title_full_relative_basename_and_status_modes() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().canonicalize().unwrap().join("src");
    std::fs::create_dir(&source).unwrap();
    let path = source.join("main.rs");
    std::fs::write(&path, "fn main() {}\n").unwrap();
    let mut a = app("", None);
    a.workspace
        .open(FileLocation {
            path: path.clone(),
            line: 1,
            column: 1,
        })
        .unwrap();
    a.workspace.folders.push(vel_search::project::Folder {
        path: dir.path().canonicalize().unwrap(),
        name: "root".into(),
        rules: Default::default(),
        follow_links: true,
    });
    a.sync_active_view();
    for (full, relative, expected) in [
        (false, false, "main.rs".to_owned()),
        (true, false, path.to_string_lossy().into_owned()),
        (true, true, "src/main.rs".into()),
        (false, true, "src/main.rs".into()),
    ] {
        set(&mut a, "show_full_path", json!(full));
        set(&mut a, "show_rel_path", json!(relative));
        assert_eq!(a.workspace.window_title(), expected);
    }
    for (mode, expected) in [
        ("enabled", "Line 1, Column 1"),
        ("compact", "Ln 1, Col 1"),
        ("disabled", ""),
    ] {
        set(&mut a, "show_line_column", json!(mode));
        a.panels.layout(a.window_size);
        let mut scene = vel_render::scene::Scene::default();
        a.panels.paint(&a.workspace, &mut scene);
        let text = scene
            .layers
            .iter()
            .flat_map(|l| &l.text)
            .map(|t| t.text.as_str())
            .collect::<Vec<_>>();
        if !expected.is_empty() {
            assert!(
                text.iter().any(|t| t.contains(expected)),
                "{mode}: {text:?}"
            );
        } else {
            assert!(
                !text
                    .iter()
                    .any(|t| t.contains("Ln 1") || t.contains("Line 1"))
            );
        }
    }
}
#[test]
fn round18_display_syntax_override_uses_the_resource_loader() {
    let profile = tempfile::tempdir().unwrap();
    let user = profile.path().join("Packages/User");
    std::fs::create_dir_all(&user).unwrap();
    std::fs::write(
        user.join("Preferences.sublime-settings"),
        r#"{"caret_extra_top":1,"block_caret":false}"#,
    )
    .unwrap();
    std::fs::write(
        user.join("Rust.sublime-settings"),
        r#"{"caret_extra_top":4,"block_caret":true}"#,
    )
    .unwrap();
    let mut a = app("fn main() {}", Some(profile.path()));
    assert!(!a.workspace.editor().settings.bool("block_caret", true));
    a.workspace
        .set_file_type("Packages/Rust/Rust.sublime-syntax")
        .unwrap();
    assert!(a.workspace.editor().settings.bool("block_caret", false));
    assert_eq!(
        a.workspace.editor().settings.number("caret_extra_top", 0.),
        4.
    );
    let scene = a.view.scene(a.workspace.editor());
    assert!(
        scene.layers[0]
            .rects
            .iter()
            .any(|r| r.rect.width == a.view.cell)
    );
    assert!(
        scene.layers[1]
            .text
            .iter()
            .any(|run| run.clip.width == a.view.cell)
    );
}
#[test]
fn round18_tabs_fit_scroll_dirty_colors_buttons_and_live_hiding() {
    let mut a = app("dirty", None);
    for _ in 0..12 {
        named(&mut a, "new_file", json!({}));
    }
    named(&mut a, "insert", json!({"characters":"x"}));
    let label_color = |a: &mut App| {
        let scene = a.scene();
        scene
            .layers
            .iter()
            .flat_map(|l| &l.text)
            .find(|t| t.text.contains('•'))
            .map(|t| t.color)
    };
    set(&mut a, "enable_tab_scrolling", json!(false));
    let before = label_color(&mut a);
    assert_eq!(a.tab_hits.len(), a.workspace.tabs.len());
    assert_eq!(a.tab_bars[0].2, 0.);
    set(&mut a, "highlight_modified_tabs", json!(true));
    let after = label_color(&mut a);
    assert_ne!(before, after);
    set(&mut a, "highlight_modified_tabs", json!(false));
    assert_eq!(label_color(&mut a), before);
    set(&mut a, "enable_tab_scrolling", json!(true));
    a.scene();
    assert!(a.tab_bars[0].2 > 0.);
    for key_name in ["hide_new_tab_button", "hide_tab_scrolling_buttons"] {
        set(&mut a, key_name, json!(false));
        a.scene();
        let class = if key_name == "hide_new_tab_button" {
            "new_tab_button"
        } else {
            "scroll_tabs_right_button"
        };
        assert!(
            a.tab_button_hits.iter().any(|(_, c, _)| *c == class),
            "{class}"
        );
        set(&mut a, key_name, json!(true));
        a.scene();
        assert!(!a.tab_button_hits.iter().any(|(_, c, _)| *c == class));
        set(&mut a, key_name, json!(false));
    }
    a.scene();
    let rect = a
        .tab_button_hits
        .iter()
        .find(|(_, c, _)| *c == "scroll_tabs_right_button")
        .unwrap()
        .2;
    for _ in 0..3 {
        mouse(
            &mut a,
            Point {
                x: rect.x + rect.width / 2.,
                y: rect.y + rect.height / 2.,
            },
            1,
        );
        a.scene();
    }
    assert!(a.workspace.workbench.groups[0].tab_scroll > 0.);
    for _ in 0..70 {
        named(&mut a, "new_file", json!({}));
    }
    set(&mut a, "enable_tab_scrolling", json!(false));
    a.scene();
    assert_eq!(a.tab_hits.len(), a.workspace.tabs.len());
    assert!(a.tab_hits.iter().all(|(_, r)| r.width > 0.));
    for (sheet, close) in &a.tab_close_hits {
        let tab = a.tab_hits.iter().find(|(id, _)| id == sheet).unwrap().1;
        assert!(close.x >= tab.x && close.x + close.width <= tab.x + tab.width);
    }
}
#[test]
fn round18_build_dispatch_panel_save_all_restart_cancel_and_key_context() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().canonicalize().unwrap().join("source");
    std::fs::write(&path, "old").unwrap();
    let mut a = app("", None);
    a.workspace
        .open(FileLocation {
            path: path.clone(),
            line: 1,
            column: 1,
        })
        .unwrap();
    a.sync_active_view();
    named(&mut a, "insert", json!({"characters":"new"}));
    set(&mut a, "show_panel_on_build", json!(false));
    named(&mut a, "build", json!({"cmd":["/bin/cat","$file"]}));
    finish(&mut a);
    assert_eq!(a.panels.panel, None);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "newold");
    assert!(a.panels.output.starts_with("newold"));
    assert!(a.panels.output.contains("[Finished in "));
    assert!(!a.panels.output.contains("[dir:"));
    set(&mut a, "save_all_on_build", json!(false));
    named(&mut a, "insert", json!({"characters":"x"}));
    set(&mut a, "show_panel_on_build", json!(true));
    named(
        &mut a,
        "exec",
        json!({"cmd":["/bin/cat","$file"],"quiet":true}),
    );
    finish(&mut a);
    assert_eq!(a.panels.panel, Some(crate::panels::Panel::Output));
    assert_eq!(a.panels.output, "newold");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "newold");
    named(&mut a, "exec", json!({"cmd":["/bin/sleep","30"]}));
    assert!(key(&mut a, "ctrl+c"));
    finish(&mut a);
    assert!(a.panels.output.contains("[Cancelled]"));
    assert!(
        !key(&mut a, "ctrl+c"),
        "cancel binding must release Ctrl+C after completion"
    );
    named(&mut a, "exec", json!({"cmd":["/bin/sleep","30"]}));
    named(&mut a, "exec", json!({"cmd":["/bin/echo","replacement"]}));
    finish(&mut a);
    assert!(a.panels.output.starts_with("replacement\n"));
    a.workspace.build_systems =
        vec![vel_build::BuildSystem::parse("fixture", r#"{"cmd":["/bin/echo","f7"]}"#).unwrap()];
    a.workspace.build_system = Some("fixture".into());
    assert!(key(&mut a, "f7"));
    finish(&mut a);
    assert!(a.panels.output.starts_with("f7\n"));
    named(&mut a, "new_build_system", json!({}));
    assert!(a.workspace.tabs[a.workspace.active].path.is_none());
    assert_eq!(
        a.workspace.tabs[a.workspace.active].name(),
        "untitled.sublime-build"
    );
    assert!(a.workspace.editor().text.to_string().contains("shell_cmd"));
}
#[test]
fn round18_build_with_choices_filter_and_remember_variants() {
    let mut a = app("fn main() {}", None);
    a.workspace
        .set_file_type("Packages/Rust/Rust.sublime-syntax")
        .unwrap();
    a.workspace.build_systems=vec![
        vel_build::BuildSystem::parse("A",r#"{"selector":"source.rust","cmd":["/bin/echo","A"],"variants":[{"name":"Run","cmd":["/bin/echo","run"]}]}"#).unwrap(),
        vel_build::BuildSystem::parse("B",r#"{"selector":"source.rust - comment","cmd":["/bin/echo","B"]}"#).unwrap(),
        vel_build::BuildSystem::parse("No",r#"{"selector":"source.python","cmd":["false"]}"#).unwrap(),
    ];
    let choices = crate::build::Builds::choices(&a.workspace, true);
    assert_eq!(choices.len(), 3);
    assert!(!choices.iter().any(|c| c.caption == "No"));
    named(&mut a, "build", json!({}));
    assert!(a.builds.task.is_none());
    assert!(a.panels.overlay.is_some());
    let _ = key(&mut a, "escape");
    named(&mut a, "build", json!({"select":true}));
    a.panels.quick_input.set("Run");
    let viewport = a.viewport();
    a.panels.edited(&mut a.workspace, viewport);
    let deadline = Instant::now() + Duration::from_secs(3);
    while a.panels.quick.items.is_empty() {
        let viewport = a.viewport();
        a.panels.poll(&mut a.workspace, viewport);
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(key(&mut a, "enter"));
    finish(&mut a);
    assert!(a.panels.output.starts_with("run\n"));
    named(&mut a, "build", json!({}));
    finish(&mut a);
    assert!(a.panels.output.starts_with("run\n"));
    named(&mut a, "set_build_system", json!({"name":"Automatic"}));
    assert!(a.builds.choice.is_none());
}
#[test]
fn round18_build_output_format_table_and_unsupported_stdin() {
    for (code, cancel, quiet, expected) in [
        (Some(0), false, false, "\n[Finished in 300ms]\n"),
        (
            Some(7),
            false,
            false,
            "\n[Finished in 300ms with exit code 7]\n",
        ),
        (None, true, false, "\n[Cancelled]\n"),
        (Some(0), false, true, ""),
        (Some(1), false, true, ""),
        (None, true, true, "\n[Cancelled]\n"),
    ] {
        assert_eq!(
            crate::build::finish_line(code, cancel, Duration::from_millis(300), quiet),
            expected
        );
    }
    let mut a = app("", None);
    named(
        &mut a,
        "exec",
        json!({"cmd":["/bin/cat"],"interactive":true}),
    );
    assert!(a.builds.task.is_none());
    assert!(
        a.panels
            .output
            .contains("Interactive builds are not supported")
    );
}

#[test]
fn round18_menu_palette_rows_resolve_against_the_lane_a_command_contract() {
    let a = app("", None);
    // Resolve the actual merged appearance commands; no registry stubs.
    for command in ["select_color_scheme", "select_theme"] {
        assert_eq!(
            a.workspace.editor().registry.kind(command),
            Some(vel_commands::Kind::WindowCommand)
        );
    }
    fn rows<'a>(
        items: &'a [vel_config::ui::MenuItem],
        prefix: &str,
        out: &mut Vec<(String, &'a str, Value)>,
    ) {
        for item in items {
            let path = if prefix.is_empty() {
                item.caption.clone()
            } else {
                format!("{prefix}/{}", item.caption)
            };
            if let Some(command) = &item.command {
                out.push((path.clone(), command, item.args.clone()));
            }
            rows(&item.children, &path, out);
        }
    }
    let mut actual = vec![];
    rows(&a.workspace.menus, "", &mut actual);
    for (path, command, args) in [
        (
            "Goto/Goto Anything…",
            "show_overlay",
            json!({"overlay":"goto"}),
        ),
        (
            "Goto/Goto Symbol…",
            "show_overlay",
            json!({"overlay":"goto","text":"@"}),
        ),
        (
            "Goto/Goto Symbol in Project…",
            "goto_symbol_in_project",
            json!({}),
        ),
        ("Goto/Goto Definition", "goto_definition", json!({})),
        ("Goto/Goto Reference", "goto_reference", json!({})),
        ("Goto/Goto Line…", "goto_line", json!({})),
        ("Goto/Jump Back", "jump_back", json!({})),
        ("Goto/Jump Forward", "jump_forward", json!({})),
        ("Goto/Switch File/Next File", "next_view", json!({})),
        ("Goto/Switch File/Previous File", "prev_view", json!({})),
        (
            "View/Preferences/Color Scheme…",
            "select_color_scheme",
            json!({}),
        ),
        ("View/Preferences/Theme…", "select_theme", json!({})),
        (
            "Tools/Build System/New Build System…",
            "new_build_system",
            json!({}),
        ),
    ] {
        let (_, found, found_args) = actual
            .iter()
            .find(|r| r.0 == path)
            .unwrap_or_else(|| panic!("missing {path}"));
        assert_eq!(*found, command);
        assert_eq!(found_args, &args);
        assert!(
            a.workspace.editor().registry.kind(command).is_some(),
            "{command}"
        );
    }
    for (caption, command) in [
        ("UI: Select Color Scheme", "select_color_scheme"),
        ("UI: Select Theme", "select_theme"),
    ] {
        assert!(
            a.workspace
                .palette
                .iter()
                .any(|p| p.caption == caption && p.invocation.command == command)
        );
    }
}
#[test]
fn round18_goto_line_keyboard_animation_keeps_the_final_center_and_selection() {
    let mut a = app(&"line\n".repeat(6000), None);
    set(&mut a, "scroll_speed", json!(1.));
    a.view.scroll_y = 0.;
    named(&mut a, "goto_line", json!({}));
    a.panels.quick_input.set(":4000");
    let viewport = a.viewport();
    a.panels.edited(&mut a.workspace, viewport);
    let caret = a.workspace.editor().caret;
    a.scene();
    assert!(a.view.scrolling());
    assert!(a.view.scroll_y < 1000.);
    a.view.advance_scroll(
        a.workspace.editor(),
        Instant::now() + Duration::from_secs(2),
    );
    assert!(a.view.scroll_y > 3900. * a.view.line_height);
    assert_eq!(a.workspace.editor().caret, caret);
}

#[test]
fn round18_accepting_a_status_bar_syntax_row_preserves_the_parked_scroll() {
    let mut a = app(&"line\n".repeat(6000), None);
    a.view.scroll_y = 5000. * a.view.line_height;
    let before = a.view.scroll_position();
    let status = Point {
        x: a.window_size.width - 90.,
        y: a.window_size.height - 1.,
    };
    mouse(&mut a, status, 1);
    assert_eq!(a.view.scroll_position(), before, "after opening");
    assert_eq!(a.panels.overlay, Some(crate::panels::Overlay::Syntax));
    a.panels.quick_input.set("Rust");
    let viewport = a.viewport();
    a.panels.edited(&mut a.workspace, viewport);
    let deadline = Instant::now() + Duration::from_secs(3);
    while a.panels.quick.items.is_empty() {
        let viewport = a.viewport();
        a.panels.poll(&mut a.workspace, viewport);
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(a.view.scroll_position(), before, "after filtering");
    a.scene();
    assert_eq!(a.view.scroll_position(), before, "after scene");
    let row = a
        .panels
        .tree
        .nodes
        .values()
        .find(|n| n.class == "quick_panel_row" && n.visible)
        .map(|n| n.rect)
        .expect("syntax row");
    let position = Point {
        x: row.x + 20.,
        y: row.y + row.height / 2.,
    };
    for kind in [MouseKind::Down, MouseKind::Up] {
        a.handle_mouse_event(
            None,
            MouseEvent {
                position,
                kind,
                button: 0,
                click_count: 1,
                modifiers: Modifiers(0),
                timestamp: 0.,
            },
        );
        assert_eq!(a.view.scroll_position(), before, "after {kind:?}");
    }
    assert!(a.panels.overlay.is_none());
    assert_eq!(a.view.scroll_position(), before);
    assert_eq!(
        a.workspace
            .editor()
            .language
            .borrow()
            .as_ref()
            .unwrap()
            .name,
        "Rust"
    );
}

#[test]
fn round18_build_selector_adapter_synchronously_scores_stale_embedded_scopes() {
    let text = "<script>let value = \"text\";</script>";
    let mut a = app(text, None);
    a.workspace
        .set_file_type("Packages/HTML/HTML.sublime-syntax")
        .unwrap();
    a.workspace
        .editor_mut()
        .set_caret(text.find("text").unwrap() + 1, false);
    assert!(
        a.workspace
            .editor()
            .language
            .borrow()
            .as_ref()
            .unwrap()
            .scope_snapshot_at(a.workspace.editor().caret)
            .is_none()
    );
    let scopes = crate::build::Builds::caret_scopes(&a.workspace);
    let names = scopes.iter().map(|s| s.build_string()).collect::<Vec<_>>();
    assert!(
        names.iter().any(|s| s.starts_with("source.js")),
        "{names:?}"
    );
    a.workspace.build_systems = vec![
        vel_build::BuildSystem::parse(
            "excluded",
            r#"{"selector":"source.js - string","cmd":["true"]}"#,
        )
        .unwrap(),
        vel_build::BuildSystem::parse(
            "specific",
            r#"{"selector":"text.html source.js string","cmd":["true"]}"#,
        )
        .unwrap(),
        vel_build::BuildSystem::parse("general", r#"{"selector":"source.js","cmd":["true"]}"#)
            .unwrap(),
    ];
    assert_eq!(
        crate::build::Builds::matching(&a.workspace)
            .iter()
            .map(|b| b.name.as_str())
            .collect::<Vec<_>>(),
        vec!["specific", "general"]
    );
}

#[test]
#[ignore = "explicit ragged build-output retention/streaming timing"]
fn round18_ragged_build_output_timing_is_incremental_bounded_and_navigable() {
    let mut a = app("", None);
    let program = r#"BEGIN { for(i=1;i<=240000;i++) printf "fixture.rs:%d:1: %08d %s\n",i,i,substr("abcdefghijklmnopqrstuvwxyz",1,i%26+1) }"#;
    named(
        &mut a,
        "exec",
        json!({"cmd":["/usr/bin/awk",program],"quiet":true,"file_regex":"^([^:]+):([0-9]+):([0-9]+): (.*)$"}),
    );
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut samples = vec![];
    let mut incremental = false;
    while a.builds.task.is_some() {
        let at = Instant::now();
        if a.builds.poll(&mut a.panels) {
            samples.push(at.elapsed().as_secs_f64() * 1000.);
        }
        incremental |= a.builds.task.is_some() && !a.panels.output.is_empty();
        assert!(a.panels.output.len() <= 8 * 1024 * 1024);
        assert!(a.builds.diagnostics.len() <= 32768);
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(incremental);
    assert!(a.panels.message.contains("240000 errors"));
    assert!(a.panels.output.starts_with("[output truncated]"));
    let lines = a.panels.output.lines().collect::<Vec<_>>();
    for diagnostic in &a.builds.diagnostics {
        assert!(
            lines[diagnostic.output_line]
                .starts_with(&format!("fixture.rs:{}:1:", diagnostic.line))
        );
    }
    samples.sort_by(f64::total_cmp);
    eprintln!(
        "ROUND18_BUILD_OUTPUT polls={} p50_ms={:.3} p95_ms={:.3} max_ms={:.3} retained_bytes={} diagnostics={}",
        samples.len(),
        samples[samples.len() / 2],
        samples[samples.len() * 95 / 100],
        samples[samples.len() - 1],
        a.panels.output.len(),
        a.builds.diagnostics.len()
    );
}

#[test]
fn round18_keyboard_scroll_interruption_and_live_zero_speed_converge() {
    let mut a = app(&"line\n".repeat(6000), None);
    named(&mut a, "move", json!({"by":"pages","forward":true}));
    assert!(a.view.scrolling());
    named(&mut a, "move", json!({"by":"characters","forward":true}));
    assert!(!a.view.scrolling());
    let position = a.view.scroll_position();
    a.view.advance_scroll(
        a.workspace.editor(),
        Instant::now() + Duration::from_secs(2),
    );
    assert_eq!(a.view.scroll_position(), position);
    named(&mut a, "move", json!({"by":"pages","forward":true}));
    assert!(a.view.scrolling());
    set(&mut a, "scroll_speed", json!(0.));
    a.view.advance_scroll(a.workspace.editor(), Instant::now());
    assert!(!a.view.scrolling());
    let caret = a
        .view
        .rect_at(a.workspace.editor(), a.workspace.editor().caret);
    assert!(caret.y >= a.view.tab_height && caret.y + caret.height <= a.view.size.height);
}

#[test]
fn round18_review_build_memory_tracks_matching_sets_and_only_picker_choices() {
    let mut a = app("fn main() {}", None);
    let rust = || {
        vel_build::BuildSystem::from_value(
            "Rust",
            json!({"selector":"source.rust","cmd":["/bin/echo","RUST"],"quiet":true}),
        )
        .unwrap()
    };
    let python = || {
        vel_build::BuildSystem::from_value(
            "Python",
            json!({"selector":"source.python","cmd":["/bin/echo","PYTHON"],"quiet":true}),
        )
        .unwrap()
    };
    a.workspace.build_systems = vec![rust(), python()];
    set_syntax(&mut a, "Rust/Rust");
    assert!(key(&mut a, "super+b"));
    finish(&mut a);
    assert_eq!(a.panels.output, "RUST\n");
    assert!(a.builds.choice.is_none());
    set_syntax(&mut a, "Python/Python");
    assert!(key(&mut a, "super+b"));
    finish(&mut a);
    assert_eq!(a.panels.output, "PYTHON\n");
    assert!(a.builds.choice.is_none());
    a.workspace.build_systems.push(
        vel_build::BuildSystem::from_value(
            "RustB",
            json!({"selector":"source.rust","cmd":["/bin/echo","RUSTB"],"quiet":true}),
        )
        .unwrap(),
    );
    set_syntax(&mut a, "Rust/Rust");
    assert!(key(&mut a, "super+b"));
    assert!(a.builds.task.is_none());
    choose_build(&mut a, "RustB");
    finish(&mut a);
    assert_eq!(a.panels.output, "RUSTB\n");
    set_syntax(&mut a, "Python/Python");
    assert!(key(&mut a, "super+b"));
    finish(&mut a);
    assert_eq!(a.panels.output, "PYTHON\n");
    set_syntax(&mut a, "Rust/Rust");
    assert!(key(&mut a, "super+b"));
    finish(&mut a);
    assert_eq!(a.panels.output, "RUSTB\n");
    a.workspace.build_systems.retain(|b| b.name != "RustB");
    assert!(key(&mut a, "super+b"));
    finish(&mut a);
    assert_eq!(a.panels.output, "RUST\n");
    set_syntax(&mut a, "Python/Python");
    named(&mut a, "set_build_system", json!({"name":"Rust"}));
    assert!(key(&mut a, "super+b"));
    finish(&mut a);
    assert_eq!(a.panels.output, "RUST\n");
}
fn set_syntax(a: &mut App, syntax: &str) {
    named(
        a,
        "set_file_type",
        json!({"syntax":format!("Packages/{syntax}.sublime-syntax")}),
    );
}
fn choose_build(a: &mut App, query: &str) {
    a.panels.quick_input.set(query);
    let viewport = a.viewport();
    a.panels.edited(&mut a.workspace, viewport);
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let viewport = a.viewport();
        a.panels.poll(&mut a.workspace, viewport);
        if !a.panels.quick.items.is_empty() {
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(key(a, "enter"));
}
fn caret_visible(a: &App) -> bool {
    let rect = a
        .view
        .rect_at(a.workspace.editor(), a.workspace.editor().caret);
    rect.y >= a.view.tab_height + 4. && rect.y + rect.height <= a.view.size.height - 4. + 0.001
}
#[test]
fn round18_review_layout_settings_anchor_wrapped_text_and_keep_visible_caret() {
    let mut a = app(&format!("{}\n", "word ".repeat(50)).repeat(5000), None);
    set(&mut a, "scroll_speed", json!(0.));
    for (name, args) in [
        ("toggle_setting", json!({"setting":"word_wrap"})),
        ("set_setting", json!({"setting":"wrap_width","value":40})),
        ("set_setting", json!({"setting":"font_size","value":20})),
        (
            "set_setting",
            json!({"setting":"line_padding_top","value":5}),
        ),
        (
            "set_setting",
            json!({"setting":"line_padding_bottom","value":3}),
        ),
        ("toggle_setting", json!({"setting":"draw_centered"})),
        ("toggle_setting", json!({"setting":"gutter"})),
        ("set_setting", json!({"setting":"rulers","value":[40,80]})),
    ] {
        // The caret is near the top so both the text anchor and visibility can be kept.
        let at = a.workspace.editor().text.line_to_byte(3000);
        a.workspace.editor_mut().set_caret(at, false);
        a.view.center_caret(a.workspace.editor());
        let row = a
            .workspace
            .editor()
            .layout
            .borrow()
            .visual_at(&a.workspace.editor().text, at)
            .0;
        a.view.scroll_y =
            (row.saturating_sub(2)) as f64 * a.view.line_height + 0.25 * a.view.line_height;
        let before = a.view.text_scroll_anchor(a.workspace.editor());
        assert!(caret_visible(&a));
        named(&mut a, name, args.clone());
        a.scene();
        let after = a.view.text_scroll_anchor(a.workspace.editor());
        assert_eq!(
            a.workspace.editor().text.byte_to_line(after.byte),
            a.workspace.editor().text.byte_to_line(before.byte),
            "{name} {args}"
        );
        assert!(caret_visible(&a), "{name} {args}");
        // A parked caret remains parked; changing units must preserve the viewed text.
        a.workspace.editor_mut().set_caret(0, false);
        let before = a.view.text_scroll_anchor(a.workspace.editor());
        named(&mut a, "toggle_setting", json!({"setting":"word_wrap"}));
        a.scene();
        let after = a.view.text_scroll_anchor(a.workspace.editor());
        assert_eq!(
            a.workspace.editor().text.byte_to_line(after.byte),
            a.workspace.editor().text.byte_to_line(before.byte)
        );
        assert!(!caret_visible(&a));
    }
}
#[test]
fn round18_review_find_enter_reveals_the_same_match_after_wheel_scroll() {
    let mut a = app(&format!("needle\n{}", "line\n".repeat(6000)), None);
    named(
        &mut a,
        "show_panel",
        json!({"panel":"find","text":"needle"}),
    );
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let viewport = a.viewport();
        a.panels.poll(&mut a.workspace, viewport);
        assert!(key(&mut a, "enter"));
        if a.workspace.editor().selection() == (0..6) {
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(2));
    }
    for chord in ["enter", "shift+enter", "enter"] {
        a.view
            .scroll(a.workspace.editor(), 0., 5000. * a.view.line_height);
        assert!(!caret_visible(&a));
        assert!(key(&mut a, chord));
        assert!(caret_visible(&a), "{chord}");
        assert_eq!(a.workspace.editor().selection(), 0..6);
    }
}
#[test]
fn round18_review_typing_and_ime_reveals_finish_page_animation() {
    for speed in [0., 1.] {
        let mut a = app(&"line\n".repeat(6000), None);
        set(&mut a, "scroll_speed", json!(speed));
        let at = a
            .workspace
            .editor()
            .text
            .line_to_byte(a.view.rows().saturating_sub(2));
        a.workspace.editor_mut().set_caret(at, false);
        assert!(key(&mut a, "pagedown"));
        // The headless route invokes the same text_input + changed_state adapter as input().
        for index in 0..6 {
            let input = if index % 2 == 0 {
                TextInput::Insert {
                    text: "\n".into(),
                    replacement: None,
                }
            } else {
                TextInput::Marked {
                    text: "\n".into(),
                    selected: Utf16Range {
                        location: 1,
                        length: 0,
                    },
                    replacement: None,
                }
            };
            a.workspace.editor_mut().text_input(input);
            a.changed_state(Instant::now(), true);
        }
        a.view.advance_scroll(
            a.workspace.editor(),
            Instant::now() + Duration::from_secs(3),
        );
        assert!(!a.view.scrolling());
        assert!(caret_visible(&a), "speed {speed}");
    }
}
#[test]
fn round18_review_tab_and_group_commands_preserve_destination_viewports() {
    let mut a = app(&"line\n".repeat(6000), None);
    named(&mut a, "new_file", json!({}));
    named(
        &mut a,
        "insert",
        json!({"characters":"line\n".repeat(6000)}),
    );
    a.workspace.editor_mut().set_caret(0, false);
    a.view.scroll_y = 4000. * a.view.line_height;
    let parked = a.view.scroll_y;
    for (away, back) in [("prev_view", "next_view"), ("next_view", "prev_view")] {
        let sheet = a.workspace.tabs[a.workspace.active].sheet;
        named(&mut a, away, json!({}));
        assert_ne!(
            a.workspace.tabs[a.workspace.active].sheet, sheet,
            "{away} must switch"
        );
        named(&mut a, back, json!({}));
        assert_eq!(
            a.workspace.tabs[a.workspace.active].sheet, sheet,
            "{back} must switch back"
        );
        assert_eq!(a.view.scroll_y, parked, "{away}/{back}");
    }
    named(&mut a, "select_by_index", json!({"index":0}));
    assert_eq!(a.workspace.active, 0);
    named(&mut a, "select_by_index", json!({"index":1}));
    assert_eq!(a.workspace.active, 1);
    assert_eq!(a.view.scroll_y, parked);
    named(&mut a, "set_layout", json!({"preset":"columns2"}));
    named(&mut a, "move_to_group", json!({"group":1}));
    named(&mut a, "focus_group", json!({"group":0}));
    named(&mut a, "focus_group", json!({"group":1}));
    assert_eq!(a.view.scroll_y, parked);
}
#[test]
fn round18_review_save_all_is_a_persisted_user_preference_and_exec_does_not_save() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let file = root.join("source");
    std::fs::write(&file, "old").unwrap();
    let mut a = app("", Some(&root));
    a.workspace
        .open(FileLocation {
            path: file.clone(),
            line: 1,
            column: 1,
        })
        .unwrap();
    a.sync_active_view();
    named(&mut a, "insert", json!({"characters":"new"}));
    named(
        &mut a,
        "exec",
        json!({"cmd":["/bin/cat","$file"],"quiet":true}),
    );
    finish(&mut a);
    assert_eq!(a.panels.output, "old");
    assert!(a.workspace.tabs[a.workspace.active].dirty());
    named(
        &mut a,
        "toggle_setting",
        json!({"setting":"save_all_on_build"}),
    );
    named(&mut a, "new_file", json!({}));
    named(&mut a, "insert", json!({"characters":"untitled"}));
    assert!(
        !a.workspace
            .editor()
            .settings
            .bool("save_all_on_build", true)
    );
    named(
        &mut a,
        "build",
        json!({"cmd":["/bin/echo","run"],"quiet":true}),
    );
    finish(&mut a);
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "old");
    a.workspace.wait_resources();
    let source =
        std::fs::read_to_string(root.join("Packages/User/Preferences.sublime-settings")).unwrap();
    assert_eq!(
        vel_config::json::Document::parse(&source).unwrap().value["save_all_on_build"],
        false
    );
    let restored = app("", Some(&root));
    assert!(
        !restored
            .workspace
            .editor()
            .settings
            .bool("save_all_on_build", true)
    );
    named(
        &mut a,
        "toggle_setting",
        json!({"setting":"save_all_on_build"}),
    );
    named(
        &mut a,
        "build",
        json!({"cmd":["/bin/echo","run"],"quiet":true}),
    );
    finish(&mut a);
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "newold");
    assert!(a.workspace.tabs[a.workspace.active].path.is_none());
}
#[test]
fn round18_review_build_context_does_not_mutate_settings_and_works_in_find_panel() {
    let mut a = app("line", None);
    named(&mut a, "exec", json!({"cmd":["/bin/sleep","30"]}));
    named(&mut a, "show_panel", json!({"panel":"find"}));
    let revision = a.workspace.editor().settings.revision();
    assert!(key(&mut a, "ctrl+c"));
    assert_eq!(a.workspace.editor().settings.revision(), revision);
    finish(&mut a);
    assert!(a.panels.output.contains("[Cancelled]"));
    assert!(
        !a.workspace
            .editor()
            .settings
            .bool("velocity_build_running", false)
    );
}
#[test]
fn round18_review_signal_output_and_retention_hysteresis() {
    let mut a = app("", None);
    named(&mut a, "exec", json!({"cmd":["/bin/sh","-c","kill -9 $$"]}));
    finish(&mut a);
    assert!(
        a.panels.output.contains("with signal 9 (Killed)"),
        "{}",
        a.panels.output
    );
    named(
        &mut a,
        "exec",
        json!({"cmd":["/usr/bin/awk","BEGIN { for(i=0;i<200000;i++) print \"012345678901234567890123456789012345678901234567890123456789\" }"],"quiet":true}),
    );
    let mut trims = 0;
    let deadline = Instant::now() + Duration::from_secs(15);
    while a.builds.task.is_some() {
        a.panels.output_reset = false;
        a.builds.poll(&mut a.panels);
        if a.panels.output_reset {
            trims += 1;
            assert!(a.panels.output.starts_with("[output truncated]"));
            // A transport reset publishes its retained tail after production
            // may have advanced since trimming. Its cap is 8 MiB; a synchronous
            // UI trim still immediately leaves at most 6 MiB.
            let transported = a
                .panels
                .output
                .lines()
                .next()
                .is_some_and(|line| line.contains("bytes dropped"));
            let limit = if transported { 8 } else { 6 } * 1024 * 1024;
            assert!(a.panels.output.len() <= limit);
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!((1..=3).contains(&trims), "{trims}");
    assert!(a.panels.output.len() <= 8 * 1024 * 1024);
}

#[test]
fn round18_review_sidebar_preview_palette_cancel_does_not_commit_recent_or_session() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let file = root.join("a.txt");
    std::fs::write(&file, "preview").unwrap();
    std::fs::write(root.join("b.txt"), "b").unwrap();
    let mut a = app("", None);
    a.panels.add_folder(&mut a.workspace, root).unwrap();
    a.panels.side_visible = true;
    let deadline = Instant::now() + Duration::from_secs(3);
    while a.panels.side_rows.len() < 3 {
        let viewport = a.viewport();
        a.panels.poll(&mut a.workspace, viewport);
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(2));
    }
    a.panels.layout(a.window_size);
    let i = a
        .panels
        .side_rows
        .iter()
        .position(|r| r.path == file)
        .unwrap();
    let count = if a.workspace.is_empty() {
        0
    } else {
        a.workspace.tabs.len()
    };
    mouse(
        &mut a,
        Point {
            x: 60.,
            y: 30. + (count + 2 + i) as f64 * 24. + 12.,
        },
        1,
    );
    assert_eq!(
        a.workspace.tabs[a.workspace.active].path.as_ref(),
        Some(&file)
    );
    let preview = a.workspace.tabs[a.workspace.active].sheet;
    assert!(a.workspace.tabs[a.workspace.active].transient);
    named(&mut a, "show_overlay", json!({"overlay":"command_palette"}));
    assert!(!a.workspace.recent_files.contains(&file));
    let (session, _) = a.workspace.snapshot().unwrap();
    assert!(!session.sheets.iter().any(|s| s.id == preview));
    assert!(key(&mut a, "escape"));
    assert!(!a.workspace.recent_files.contains(&file));
    assert!(a.workspace.tabs[a.workspace.active].transient);
    // An actual Goto confirmation still promotes this preview.
    named(&mut a, "show_overlay", json!({"overlay":"goto"}));
    assert!(key(&mut a, "enter"));
    assert!(a.workspace.recent_files.contains(&file));
    let (session, _) = a.workspace.snapshot().unwrap();
    assert!(session.sheets.iter().any(|s| s.id == preview));
}
#[test]
fn round18_review_sidebar_open_files_preserves_parked_scroll() {
    let mut a = app(&"line\n".repeat(6000), None);
    named(&mut a, "new_file", json!({}));
    named(
        &mut a,
        "insert",
        json!({"characters":"line\n".repeat(6000)}),
    );
    a.workspace.editor_mut().set_caret(0, false);
    a.view.scroll_y = 4000. * a.view.line_height;
    let parked = a.view.scroll_y;
    a.panels.side_visible = true;
    a.panels.layout(a.window_size);
    mouse(
        &mut a,
        Point {
            x: 60.,
            y: 30. + 24. + 12.,
        },
        1,
    );
    mouse(
        &mut a,
        Point {
            x: 60.,
            y: 30. + 48. + 12.,
        },
        1,
    );
    assert_eq!(a.workspace.active, 1);
    assert_eq!(a.view.scroll_y, parked);
}
#[test]
fn round18_review_headless_result_navigation_obeys_active_result_source() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().canonicalize().unwrap().join("results");
    std::fs::write(&file, "one\ntwo\nthree\n").unwrap();
    let mut a = app("", None);
    a.panels.result_view.push(vel_search::files::Line {
        path: file.clone(),
        number: 3,
        text: "three".into(),
        highlights: std::iter::once(0..5).collect(),
    });
    a.panels.search_results_active = true;
    named(&mut a, "next_result", json!({}));
    assert_eq!(
        a.workspace.tabs[a.workspace.active].path.as_ref(),
        Some(&file)
    );
    assert_eq!(
        a.workspace
            .editor()
            .text
            .byte_to_line(a.workspace.editor().caret),
        2
    );
    a.builds.diagnostics.push(vel_build::Diagnostic {
        path: file.clone(),
        line: 2,
        column: 1,
        message: "fixture".into(),
        output_line: 0,
    });
    a.panels.search_results_active = false;
    named(&mut a, "next_result", json!({}));
    assert_eq!(
        a.workspace
            .editor()
            .text
            .byte_to_line(a.workspace.editor().caret),
        1
    );
}
#[test]
fn round18_review_large_root_selector_build_avoids_a_full_caret_parse() {
    let mut a = app(&"fn main() {}\n".repeat(120000), None);
    set_syntax(&mut a, "Rust/Rust");
    a.workspace.build_systems = vec![
        vel_build::BuildSystem::from_value(
            "Rust",
            json!({"selector":"source.rust","cmd":["/bin/echo","ROOT"],"quiet":true}),
        )
        .unwrap(),
    ];
    let at = a.workspace.editor().text.line_to_byte(119999);
    a.workspace.editor_mut().set_caret(at, false);
    let matching = crate::build::Builds::matching(&a.workspace);
    assert_eq!(matching.len(), 1);
    assert!(
        a.workspace
            .editor()
            .language
            .borrow()
            .as_ref()
            .unwrap()
            .scope_snapshot_at(at)
            .is_none()
    );
    let requests = crate::build::MATCHING_REQUESTS.with(|n| n.get());
    assert!(key(&mut a, "f7"));
    finish(&mut a);
    assert_eq!(
        crate::build::MATCHING_REQUESTS.with(|n| n.get()),
        requests + 1
    );
    named(&mut a, "set_build_system", json!({"name":"Rust"}));
    let requests = crate::build::MATCHING_REQUESTS.with(|n| n.get());
    assert!(key(&mut a, "f7"));
    finish(&mut a);
    assert_eq!(a.panels.output, "ROOT\n");
    assert_eq!(crate::build::MATCHING_REQUESTS.with(|n| n.get()), requests);
}

#[test]
#[ignore = "120,000-line automatic/explicit build-dispatch timing"]
fn round18_review_build_matching_120000_lines_timing() {
    let mut a = app(&"fn main() {}\n".repeat(120000), None);
    set_syntax(&mut a, "Rust/Rust");
    let at = a.workspace.editor().text.line_to_byte(119999);
    a.workspace.editor_mut().set_caret(at, false);
    a.workspace.build_systems = vec![
        vel_build::BuildSystem::from_value(
            "Rust",
            json!({"selector":"source.rust","cmd":["/bin/true"]}),
        )
        .unwrap(),
    ];
    let mut root = vec![];
    let mut embedded = vec![];
    for _ in 0..9 {
        let at = Instant::now();
        std::hint::black_box(crate::build::Builds::matching(&a.workspace));
        root.push(at.elapsed().as_secs_f64() * 1000.);
    }
    a.workspace.build_systems.push(
        vel_build::BuildSystem::from_value(
            "Embedded",
            json!({"selector":"source.rust string","cmd":["/bin/true"]}),
        )
        .unwrap(),
    );
    for _ in 0..9 {
        let at = Instant::now();
        std::hint::black_box(crate::build::Builds::matching(&a.workspace));
        embedded.push(at.elapsed().as_secs_f64() * 1000.);
    }
    root.sort_by(f64::total_cmp);
    embedded.sort_by(f64::total_cmp);
    println!(
        "BUILD_MATCHING_120000 root_median_ms={} embedded_budget_median_ms={} embedded_max_ms={}",
        root[4], embedded[4], embedded[8]
    );
}

#[test]
fn round18_review_syntax_layout_override_preserves_reading_position() {
    let profile = tempfile::tempdir().unwrap();
    let user = profile.path().join("Packages/User");
    std::fs::create_dir_all(&user).unwrap();
    std::fs::write(
        user.join("Rust.sublime-settings"),
        r#"{"word_wrap":false,"font_size":17,"line_padding_top":3}"#,
    )
    .unwrap();
    let mut a = app(
        &format!("{}\n", "word ".repeat(50)).repeat(5000),
        Some(profile.path()),
    );
    let at = a.workspace.editor().text.line_to_byte(3000);
    a.workspace.editor_mut().set_caret(at, false);
    a.view.center_caret(a.workspace.editor());
    let visual = a
        .workspace
        .editor()
        .layout
        .borrow()
        .visual_at(&a.workspace.editor().text, at)
        .0;
    a.view.scroll_y = visual.saturating_sub(2) as f64 * a.view.line_height;
    let before = a.view.text_scroll_anchor(a.workspace.editor());
    set_syntax(&mut a, "Rust/Rust");
    a.scene();
    let after = a.view.text_scroll_anchor(a.workspace.editor());
    assert_eq!(
        a.workspace.editor().text.byte_to_line(after.byte),
        a.workspace.editor().text.byte_to_line(before.byte)
    );
    assert!(caret_visible(&a));
}

#[path = "app_lane_b_round18_verify_tests.rs"]
mod verification;
