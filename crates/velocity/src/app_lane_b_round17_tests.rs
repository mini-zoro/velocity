//! Round 17 Lane B regressions through real resource and command dispatch.
use super::*;
use crate::cli::FileLocation;
use vel_buffer::{Region, Selection};
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
    assert!(workspace.messages.is_empty(), "{:?}", workspace.messages);
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
fn named(app: &mut App, name: &str, args: Value) {
    app.run_named(None, name, &args);
}
fn key(app: &mut App, chord: &str) {
    let viewport = app.viewport();
    assert!(
        app.dispatch_key_binding(
            None,
            &vel_config::keymap::Key::parse(chord).unwrap(),
            Instant::now(),
            viewport
        ),
        "{chord}"
    );
}
#[test]
fn bundled_startup_has_no_messages_and_every_menu_and_palette_command_resolves() {
    let app = app("", None);
    // Resolve the merged command registry without stubs.

    assert!(
        !app.workspace
            .editor()
            .settings
            .bool("always_show_minimap_viewport", true)
    );
    fn visit(items: &[vel_config::ui::MenuItem], app: &App) {
        for item in items {
            if let Some(command) = &item.command
                && (command != "set_line_ending"
                    || app.workspace.editor().registry.kind(command).is_some())
            {
                assert!(
                    app.workspace.editor().registry.kind(command).is_some(),
                    "{command}"
                );
            }
            visit(&item.children, app);
        }
    }
    visit(&app.workspace.menus, &app);
    for item in &app.workspace.palette {
        // Lane A round 20 supplies this command; no stub on main.
        if item.invocation.command == "set_line_ending"
            && app
                .workspace
                .editor()
                .registry
                .kind("set_line_ending")
                .is_none()
        {
            continue;
        }
        assert!(
            app.workspace
                .editor()
                .registry
                .kind(&item.invocation.command)
                .is_some(),
            "{}",
            item.invocation.command
        );
    }
    assert!(app.workspace.messages.is_empty());
}
#[test]
fn new_open_clone_move_and_session_tabs_share_window_layers_and_separate_modes() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app("normal", Some(dir.path()));
    named(&mut app, "toggle_minimap", json!({}));
    assert!(!app.workspace.window_visible("show_minimap"));
    key(&mut app, "ctrl+super+shift+f");
    assert!(app.workspace.distraction_free);
    assert!(!app.workspace.window_visible("show_tabs"));
    named(&mut app, "toggle_minimap", json!({}));
    named(&mut app, "toggle_tabs", json!({}));
    named(&mut app, "toggle_status_bar", json!({}));
    named(&mut app, "toggle_side_bar", json!({}));
    named(&mut app, "new_file", json!({}));
    assert!(app.workspace.editor().settings.bool("draw_centered", false));
    let path = dir.path().join("opened.txt");
    std::fs::write(&path, "opened").unwrap();
    app.workspace
        .open(FileLocation {
            path,
            line: 0,
            column: 0,
        })
        .unwrap();
    named(&mut app, "clone_file", json!({}));
    named(&mut app, "set_layout", json!({"preset":"columns2"}));
    named(&mut app, "move_to_group", json!({"group":1}));
    for tab in &app.workspace.tabs {
        assert!(tab.editor.settings.bool("show_minimap", false));
        assert!(tab.editor.settings.bool("draw_centered", false));
        assert!(!tab.editor.settings.bool("line_numbers", true));
        assert!(tab.editor.settings.bool("show_tabs", false));
    }
    let (window, documents) = app.workspace.snapshot().unwrap();
    // Exercise the actual session wire format rather than copying a live object.
    let window: vel_session::Window =
        serde_json::from_slice(&serde_json::to_vec(&window).unwrap()).unwrap();
    let mut restored = Workspace::new(Text::new(), Some(dir.path().into()), None, false).unwrap();
    restored.restore_window(&window, &documents).unwrap();
    assert!(restored.distraction_free);
    assert!(
        restored
            .tabs
            .iter()
            .all(|t| t.editor.settings.bool("draw_centered", false)
                && t.editor.settings.bool("show_minimap", false))
    );
    restored
        .workbench_action("toggle_distraction_free", &json!({}))
        .unwrap();
    assert!(!restored.window_visible("show_minimap"));
    assert!(restored.window_visible("show_tabs"));
    restored.new_file().unwrap();
    assert!(!restored.editor().settings.bool("draw_centered", false));
    restored
        .workbench_action("toggle_distraction_free", &json!({}))
        .unwrap();
    assert!(restored.window_visible("show_minimap"));
    assert!(restored.window_visible("show_status_bar"));
    assert!(restored.window_visible("show_side_bar"));
    assert!(restored.messages.is_empty(), "{:?}", restored.messages);
}
#[test]
fn zoom_key_dispatch_persists_user_preference_and_reset_removes_or_uses_default() {
    let dir = tempfile::tempdir().unwrap();
    let user = dir.path().join("Packages/User");
    std::fs::create_dir_all(&user).unwrap();
    std::fs::write(
        user.join("Preferences.sublime-settings"),
        "{//keep\n\"font_size\":14.5,\"tab_size\":7}",
    )
    .unwrap();
    let mut app = app("zoom", Some(dir.path()));
    named(&mut app, "new_file", json!({}));
    key(&mut app, "super+=");
    assert!(
        app.workspace
            .tabs
            .iter()
            .all(|t| t.editor.preferences.font_size == 15.5)
    );
    assert!(app.workspace.tabs.iter().all(|t| {
        !t.editor
            .settings
            .layer(Layer::View)
            .contains_key("font_size")
    }));
    key(&mut app, "super+-");
    key(&mut app, "super+=");
    key(&mut app, "super+=");
    app.workspace.wait_resources();
    assert!(
        app.workspace
            .tabs
            .iter()
            .all(|t| t.editor.preferences.font_size == 16.5)
    );
    let source = std::fs::read_to_string(user.join("Preferences.sublime-settings")).unwrap();
    assert!(source.contains("//keep"));
    named(&mut app, "reset_font_size", json!({}));
    app.workspace.wait_resources();
    let source = std::fs::read_to_string(user.join("Preferences.sublime-settings")).unwrap();
    let parsed = vel_config::json::Document::parse(&source).unwrap();
    assert!(source.contains("//keep"));
    assert!(parsed.value.get("font_size").is_none());
    assert_eq!(parsed.value["tab_size"], 7);
    std::fs::write(
        user.join("Preferences.sublime-settings"),
        "{\"default_font_size\":17.25,\"tab_size\":7}",
    )
    .unwrap();
    app.workspace.request_resource_reload();
    app.workspace.wait_resources();
    named(&mut app, "reset_font_size", json!({}));
    app.workspace.wait_resources();
    assert!(
        app.workspace
            .tabs
            .iter()
            .all(|t| t.editor.preferences.font_size == 17.25)
    );
}
#[test]
fn indentation_menu_arguments_dispatch_and_undo_as_one_edit() {
    let mut app = app("\tx\n\t\ty\n", None);
    app.workspace
        .editor_mut()
        .settings
        .set(Layer::View, "tab_size", json!(4));
    app.workspace.editor_mut().sync();
    fn find<'a>(
        items: &'a [vel_config::ui::MenuItem],
        name: &str,
    ) -> Option<&'a vel_config::ui::MenuItem> {
        items.iter().find_map(|i| {
            if i.command.as_deref() == Some(name) {
                Some(i)
            } else {
                find(&i.children, name)
            }
        })
    }
    for command in ["expand_tabs", "unexpand_tabs", "detect_indentation"] {
        let args = find(&app.workspace.menus, command).unwrap().args.clone();
        if command != "detect_indentation" {
            assert_eq!(args["set_translate_tabs"], true);
        }
        named(&mut app, command, args);
        if command == "expand_tabs" {
            assert_eq!(
                app.workspace.editor().text.to_string(),
                "    x\n        y\n"
            );
            key(&mut app, "super+z");
            assert_eq!(app.workspace.editor().text.to_string(), "\tx\n\t\ty\n");
            named(&mut app, command, json!({"set_translate_tabs":true}));
        }
    }
    assert!(app.workspace.editor().text.to_string().starts_with('\t'));
}
#[test]
fn selected_text_drag_waits_for_the_configured_delay_through_mouse_dispatch() {
    let mut app = app("one two three", None);
    app.mouse_preferences.text_drag_delay = Some(Duration::from_millis(300));
    app.workspace
        .editor_mut()
        .set_selection(Selection::from_regions([Region::new(0, 3)]));
    let at = app.view.rect_at(app.workspace.editor(), 1);
    let end = app.view.rect_at(app.workspace.editor(), 13);
    let press = Point {
        x: app.content.x + at.x,
        y: app.content.y + at.y + 5.,
    };
    let drop = Point {
        x: app.content.x + end.x,
        y: app.content.y + end.y + 5.,
    };
    let send = |app: &mut App, position, kind| {
        app.handle_mouse_event(
            None,
            MouseEvent {
                position,
                kind,
                button: 0,
                click_count: 1,
                modifiers: Modifiers(0),
                timestamp: 0.,
            },
        )
    };
    send(&mut app, press, MouseKind::Down);
    send(&mut app, drop, MouseKind::Drag);
    assert!(app.view.drop_caret.is_none());
    assert_eq!(app.workspace.editor().text.to_string(), "one two three");
    assert_eq!(app.workspace.editor().selection(), 1..13);
    assert!(app.drag_text.is_none());
    // A late event remains a selection gesture after early movement.
    app.text_drag_pressed = Some(Instant::now() - Duration::from_millis(301));
    send(&mut app, drop, MouseKind::Drag);
    assert!(app.view.drop_caret.is_none());
    send(&mut app, drop, MouseKind::Up);
    assert_eq!(app.workspace.editor().text.to_string(), "one two three");
    app.workspace
        .editor_mut()
        .set_selection(Selection::from_regions([Region::new(0, 3)]));
    send(&mut app, press, MouseKind::Down);
    app.text_drag_pressed = Some(Instant::now() - Duration::from_millis(301));
    send(&mut app, drop, MouseKind::Drag);
    assert_eq!(app.view.drop_caret, Some(13));
    send(&mut app, drop, MouseKind::Up);
    assert_eq!(app.workspace.editor().text.to_string(), " two threeone");
}
#[test]
fn real_scheme_loader_drives_bracket_glyphs_glow_and_connected_selection_corners() {
    let dir = tempfile::tempdir().unwrap();
    let user = dir.path().join("Packages/User");
    std::fs::create_dir_all(&user).unwrap();
    std::fs::write(
        user.join("Preferences.sublime-settings"),
        r#"{"color_scheme":"Drawing.sublime-color-scheme","show_minimap":false,"word_wrap":false}"#,
    )
    .unwrap();
    std::fs::write(user.join("Drawing.sublime-color-scheme"),r##"{"globals":{"foreground":"#ddd","background":"#111","selection":"#246","selection_border":"#fff","selection_corner_style":"cut","selection_corner_radius":"3","brackets_foreground":"#ff3300","brackets_options":"foreground bold italic","bracket_contents_options":"foreground bold italic"},"rules":[{"scope":"keyword","font_style":"glow"}]}"##).unwrap();
    let mut app = app(
        "fn sample() {\n    let v = (1);\n    value\n}\n",
        Some(dir.path()),
    );
    named(&mut app, "set_file_type", json!({"syntax":"Rust"}));
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        app.workspace.poll();
        app.view.scene(app.workspace.editor());
        if app
            .workspace
            .editor()
            .language
            .borrow()
            .as_ref()
            .is_some_and(|l| l.analysis.is_some() && l.styled.values().any(|s| s.exact))
        {
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
    let at = app.workspace.editor().text.to_string().find("(1)").unwrap();
    app.workspace.editor_mut().set_caret(at, false);
    let scene = app.view.scene(app.workspace.editor());
    let layer = &scene.layers[0];
    assert!(
        layer.text.iter().any(|r| r.text == "("
            && r.style.bold
            && r.style.italic
            && r.color == [1., 0.2, 0., 1.])
    );
    assert!(layer.underlines.is_empty());
    assert!(!layer.glow.is_empty());
    assert!(layer.glow.iter().all(|g| {
        layer
            .text
            .iter()
            .any(|r| r.text == g.text && r.x == g.x && r.baseline == g.baseline && r.clip == g.clip)
    }));
    named(&mut app, "select_all", json!({}));
    let scene = app.view.scene(app.workspace.editor());
    let contours = &scene.layers[0].contours;
    assert_eq!(contours.len(), 1);
    assert!(!contours[0].triangles.is_empty());
    assert!(
        contours[0]
            .points
            .iter()
            .any(|p| p.x.fract() != 0. || p.y.fract() != 0.)
            || contours[0].points.len() > 4
    );
    assert!(app.workspace.messages.is_empty());
}

fn mouse(app: &mut App, local: Point, kind: MouseKind) {
    app.handle_mouse_event(
        None,
        MouseEvent {
            position: Point {
                x: app.content.x + local.x,
                y: app.content.y + local.y,
            },
            kind,
            button: 0,
            click_count: 1,
            modifiers: Modifiers(0),
            timestamp: 0.,
        },
    );
}
#[test]
fn minimap_drag_reaches_both_ends_without_jumping_in_long_wrapped_and_folded_documents() {
    for (wrap, folded) in [(false, false), (true, false), (true, true)] {
        let dir = tempfile::tempdir().unwrap();
        let user = dir.path().join("Packages/User");
        std::fs::create_dir_all(&user).unwrap();
        std::fs::write(
            user.join("Preferences.sublime-settings"),
            json!({
                "word_wrap":wrap,"wrap_width":12,"show_minimap":true,"scroll_past_end":true,
                "always_show_minimap_viewport":true,"overlay_scroll_bars":"disabled"
            })
            .to_string(),
        )
        .unwrap();
        let mut app = app(
            &"a long line wrapping over several rows\n".repeat(10_000),
            Some(dir.path()),
        );
        if folded {
            let e = app.workspace.editor_mut();
            e.set_folded(vec![Region::new(
                e.text.line_to_byte(100),
                e.text.line_to_byte(5000),
            )]);
            assert!(!e.folded().is_empty());
            app.workspace.editor_mut().set_caret(0, false);
        }
        for size in [
            Size {
                width: 800.,
                height: 600.,
            },
            Size {
                width: 1200.,
                height: 800.,
            },
        ] {
            app.window_size = size;
            app.content = app.panels.layout(size);
            app.view.size = Size {
                width: app.content.width,
                height: app.content.height,
            };
            app.view.scroll(app.workspace.editor(), 0., 1e9);
            let max = app.view.scroll_y;
            app.view.scroll(app.workspace.editor(), 0., -1e9);
            app.view.scene(app.workspace.editor());
            let (bounds, box_rect) = app.view.minimap_rects(app.workspace.editor());
            let grab = box_rect.height / 2.;
            let press = Point {
                x: bounds.x + 20.,
                y: box_rect.y + grab,
            };
            mouse(&mut app, press, MouseKind::Down);
            for _ in 0..30 {
                mouse(&mut app, press, MouseKind::Drag);
                assert_eq!(app.view.scroll_y, 0.);
            }
            let mut previous = 0.;
            for dy in [1., 5., 10., 20.] {
                let point = Point {
                    y: press.y + dy,
                    ..press
                };
                mouse(&mut app, point, MouseKind::Drag);
                let expected = dy / (bounds.height - box_rect.height) * max;
                assert!((app.view.scroll_y - expected).abs() < 1e-6);
                assert!(app.view.scroll_y > previous);
                previous = app.view.scroll_y;
                let (_, viewport) = app.view.minimap_rects(app.workspace.editor());
                assert!((viewport.y + grab - point.y).abs() < 1e-6);
                let scene = app.view.scene(app.workspace.editor());
                assert!(
                    scene
                        .layers
                        .iter()
                        .any(|layer| layer.rects.iter().any(|r| r.rect == viewport))
                );
                for _ in 0..30 {
                    mouse(&mut app, point, MouseKind::Drag);
                    assert!((app.view.scroll_y - previous).abs() < 1e-6);
                }
            }
            let bottom = Point {
                y: bounds.y + bounds.height - grab,
                ..press
            };
            mouse(&mut app, bottom, MouseKind::Drag);
            assert!((app.view.scroll_y - max).abs() < 1e-6);
            let (_, viewport) = app.view.minimap_rects(app.workspace.editor());
            assert!((viewport.y + grab - bottom.y).abs() < 1e-6);
            mouse(&mut app, bottom, MouseKind::Up);
            assert_eq!(app.view.minimap_rects(app.workspace.editor()).1, viewport);
            assert!(!app.view.minimap_drag);
            let last = app.view.rect_at(
                app.workspace.editor(),
                app.workspace.editor().text.len_bytes(),
            );
            assert!(
                last.y >= app.view.tab_height + 4. - 1e-6
                    && last.y + last.height <= app.view.size.height
            );
            mouse(&mut app, bottom, MouseKind::Down);
            // Both clamping probes stay inside the minimap, so panels cannot consume them.
            mouse(
                &mut app,
                Point {
                    y: bounds.y + bounds.height - 0.01,
                    ..press
                },
                MouseKind::Drag,
            );
            assert!((app.view.scroll_y - max).abs() < 1e-6);
            mouse(
                &mut app,
                Point {
                    y: bounds.y + 0.01,
                    ..press
                },
                MouseKind::Drag,
            );
            assert_eq!(app.view.scroll_y, 0.);
            mouse(&mut app, press, MouseKind::Up);
        }
    }
}
#[test]
fn scroll_past_end_and_scrollbar_drag_keep_the_last_line_visible() {
    let dir = tempfile::tempdir().unwrap();
    let user = dir.path().join("Packages/User");
    std::fs::create_dir_all(&user).unwrap();
    for value in [json!(false), json!(0.5), json!(true)] {
        std::fs::write(user.join("Preferences.sublime-settings"),json!({
            "scroll_past_end":value,"word_wrap":false,"show_minimap":false,"overlay_scroll_bars":"disabled"
        }).to_string()).unwrap();
        let mut app = app(&"line\n".repeat(500), Some(dir.path()));
        app.view.size = Size {
            width: 800.,
            height: 300.,
        };
        app.view.scroll(app.workspace.editor(), 0., 1e9);
        let end = app.view.scroll_y;
        let last = app.view.rect_at(
            app.workspace.editor(),
            app.workspace.editor().text.len_bytes(),
        );
        assert!(last.y >= app.view.tab_height + 4. - 0.001);
        assert!(last.y + last.height <= app.view.size.height);
        app.view.scroll_y = 0.;
        app.view.scene(app.workspace.editor());
        let (track, thumb) = app.view.scrollbar_rects();
        assert!(track.height > 0.);
        let press = Point {
            x: thumb.x + thumb.width / 2.,
            y: thumb.y + thumb.height / 2.,
        };
        mouse(&mut app, press, MouseKind::Down);
        mouse(
            &mut app,
            Point {
                y: track.y + track.height - 0.01,
                ..press
            },
            MouseKind::Drag,
        );
        assert_eq!(app.view.scroll_y, end);
        mouse(&mut app, press, MouseKind::Up);
    }
}
fn install_test_metrics(app: &mut App, scale: f32) {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../vel-render/fixtures/ligature-fixture.ttf");
    let mut fonts = Fonts::load_file(&path, "VelocityFixture-Regular").unwrap();
    fonts.cell = 8.;
    fonts.metrics = vel_render::typography::FontMetrics {
        ascent: 12.06689453125,
        descent: 3.06591796875,
        leading: 0.,
    };
    app.fonts = Some(fonts);
    app.workspace.themes.ui.borrow_mut().density = scale;
    app.initialize_view_metrics();
    app.view.apply_style(app.workspace.editor());
}
#[test]
fn session_scroll_migrates_old_pixels_and_round_trips_visual_rows() {
    let mut source = app(&"line\n".repeat(3000), None);
    let (mut window, documents) = source.workspace.snapshot().unwrap();
    // This wire value predates scroll_line and used a 20 px row.
    let mut old = serde_json::to_value(&window).unwrap();
    old["sheets"][0]["view"]["scroll"] = json!([3., 20_005.]);
    old["sheets"][0]["view"]
        .as_object_mut()
        .unwrap()
        .remove("scroll_line");
    window = serde_json::from_value(old).unwrap();
    for scale in [1., 2.] {
        let mut restored = app("", None);
        restored
            .workspace
            .restore_window(&window, &documents)
            .unwrap();
        restored
            .workspace
            .prepare_sheet(restored.workspace.active)
            .unwrap();
        restored.view_buffer = None;
        install_test_metrics(&mut restored, scale);
        restored.sync_active_view();
        assert!((restored.view.scroll_y / restored.view.line_height - 1000.25).abs() < 1e-6);

        let tab = &mut restored.workspace.tabs[restored.workspace.active];
        tab.scroll = (restored.view.scroll_x, restored.view.scroll_y);
        tab.scroll_line_height = restored.view.line_height;
        let (saved, _) = restored.workspace.snapshot().unwrap();
        let saved: vel_session::Window =
            serde_json::from_slice(&serde_json::to_vec(&saved).unwrap()).unwrap();
        assert_eq!(saved.sheets[0].view.scroll_line, Some(1000.25));
    }
}
#[test]
fn zoom_and_scale_preserve_active_and_background_top_rows_and_new_view_metrics() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app(&"line\n".repeat(3000), Some(dir.path()));
    install_test_metrics(&mut app, 1.);
    app.view.scroll_y = 1000.25 * app.view.line_height;
    let first = app.workspace.tabs[0].sheet;
    named(&mut app, "new_file", json!({}));
    assert_eq!(app.view.line_height, 16.);
    assert_eq!(app.panels.output_view.line_height, 16.); // style applied below for output too
    let second = app.workspace.active;
    named(
        &mut app,
        "insert",
        json!({"characters":"line\n".repeat(3000)}),
    );
    let caret = app.workspace.editor().text.line_to_byte(20);
    app.workspace
        .editor_mut()
        .set_selection(Selection::from_regions([Region::caret(caret)]));
    app.view.scroll_y = 17.5 * app.view.line_height;
    key(&mut app, "super+=");
    assert!(
        (app.view.scroll_y / app.view.line_height - 17.5).abs() < 1e-6,
        "scroll={} height={} caret={} size={:?}",
        app.view.scroll_y,
        app.view.line_height,
        app.workspace.editor().caret,
        app.view.size
    );
    app.workspace
        .activate_sheet(app.workspace.sheet_index(first).unwrap());
    app.sync_active_view();
    assert!((app.view.scroll_y / app.view.line_height - 1000.25).abs() < 1e-6);
    app.workspace.themes.ui.borrow_mut().density = 2.;
    app.sync_active_view();
    assert!((app.view.scroll_y / app.view.line_height - 1000.25).abs() < 1e-6);
    app.workspace.activate_sheet(second);
    app.sync_active_view();
    assert!(
        (app.view.scroll_y / app.view.line_height - 17.5).abs() < 1e-6,
        "scroll={} height={} caret={} size={:?}",
        app.view.scroll_y,
        app.view.line_height,
        app.workspace.editor().caret,
        app.view.size
    );
}
#[test]
fn zoom_interleaved_with_ui_writes_preserves_both_preferences_without_backups() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app("", Some(dir.path()));
    for _ in 0..5 {
        key(&mut app, "super+=");
        app.panels
            .review_action(
                &mut app.workspace,
                "persist_spelling_words",
                &json!({"key":"added_words","words":["velocityfixture"]}),
            )
            .unwrap();
        key(&mut app, "super+-");
    }
    app.workspace.wait_resources();
    let path = dir
        .path()
        .join("Packages/User/Preferences.sublime-settings");
    let value = vel_config::json::Document::parse(&std::fs::read_to_string(&path).unwrap())
        .unwrap()
        .value;
    assert_eq!(value["font_size"], 13.);
    assert_eq!(value["added_words"], json!(["velocityfixture"]));
    assert!(!path.with_extension("sublime-settings.bak").exists());
}
#[test]
fn numeric_ruler_and_distraction_zoom_use_the_real_settings_loader() {
    let dir = tempfile::tempdir().unwrap();
    let user = dir.path().join("Packages/User");
    std::fs::create_dir_all(&user).unwrap();
    std::fs::write(
        user.join("Preferences.sublime-settings"),
        r#"{"font_size":14,"rulers":79}"#,
    )
    .unwrap();
    std::fs::write(
        user.join("Distraction Free.sublime-settings"),
        r#"{"font_size":25}"#,
    )
    .unwrap();
    let mut app = app("", Some(dir.path()));
    assert_eq!(app.workspace.editor().preferences.rulers, vec![79]);
    named(&mut app, "toggle_distraction_free", json!({}));
    assert_eq!(app.workspace.editor().preferences.font_size, 25.);
    key(&mut app, "super+=");
    app.workspace.wait_resources();
    assert_eq!(app.workspace.editor().preferences.font_size, 25.);
    named(&mut app, "toggle_distraction_free", json!({}));
    assert_eq!(app.workspace.editor().preferences.font_size, 15.);
    let value = vel_config::json::Document::parse(
        &std::fs::read_to_string(user.join("Preferences.sublime-settings")).unwrap(),
    )
    .unwrap()
    .value;
    assert_eq!(value["font_size"], 15.);
}
#[test]
fn sidebar_reveal_and_add_folder_remember_the_current_window_mode() {
    for command in ["add_folder", "reveal_in_side_bar"] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("file.txt");
        std::fs::write(&path, "text").unwrap();
        let mut app = app("", None);
        app.workspace
            .open(FileLocation {
                path: path.clone(),
                line: 0,
                column: 0,
            })
            .unwrap();
        app.sync_active_view();
        named(&mut app, "toggle_distraction_free", json!({}));
        assert!(!app.workspace.window_visible("show_side_bar"));
        if command == "reveal_in_side_bar" {
            // The native context-menu dispatcher calls this same effect handler.
            app.panels
                .reveal_sidebar_path(&mut app.workspace, path)
                .unwrap();
        } else {
            named(&mut app, command, json!({"dir":dir.path()}));
        }
        assert!(app.panels.side_visible, "{command}: {}", app.panels.message);
        assert!(app.workspace.window_visible("show_side_bar"));
        named(&mut app, "toggle_distraction_free", json!({}));
        named(&mut app, "toggle_distraction_free", json!({}));
        assert!(app.panels.side_visible);
    }
}

#[test]
fn borderless_round_selection_reuses_contours_and_square_uses_rectangles() {
    let dir = tempfile::tempdir().unwrap();
    let user = dir.path().join("Packages/User");
    std::fs::create_dir_all(&user).unwrap();
    std::fs::write(
        user.join("Preferences.sublime-settings"),
        r#"{"color_scheme":"Corners.sublime-color-scheme","show_minimap":false,"word_wrap":false}"#,
    )
    .unwrap();
    std::fs::write(
        user.join("Corners.sublime-color-scheme"),
        r##"{"globals":{"foreground":"#ddd","background":"#111","selection":"#246"}}"##,
    )
    .unwrap();
    let mut app = app("line\na much longer line\nshort", Some(dir.path()));
    app.workspace.wait_resources();
    named(&mut app, "select_all", json!({}));
    let first = app.view.scene(app.workspace.editor());
    let c = &first.layers[0].contours[0];
    assert_eq!(c.border[3], 0.);
    assert!(c.points.len() > 10);
    let second = app.view.scene(app.workspace.editor());
    assert!(Arc::ptr_eq(&c.points, &second.layers[0].contours[0].points));
    assert!(Arc::ptr_eq(
        &c.triangles,
        &second.layers[0].contours[0].triangles
    ));
    app.view.scroll_y = 1.;
    let third = app.view.scene(app.workspace.editor());
    assert!(!Arc::ptr_eq(&c.points, &third.layers[0].contours[0].points));
    std::fs::write(user.join("Corners.sublime-color-scheme"),r##"{"globals":{"foreground":"#ddd","background":"#111","selection":"#246","selection_corner_style":"square"}}"##).unwrap();
    app.workspace.request_resource_reload();
    app.workspace.wait_resources();
    let square = app.view.scene(app.workspace.editor());
    assert!(square.layers[0].contours.is_empty());
}
#[test]
fn never_shown_tab_reveal_and_output_use_metrics_before_the_first_scene() {
    let mut app = app("first", None);
    install_test_metrics(&mut app, 2.);
    // Create and populate a tab before it has ever had a View or frame.
    app.workspace.new_file().unwrap();
    app.workspace
        .editor_mut()
        .run("insert", &json!({"characters":"line\n".repeat(3000)}))
        .unwrap();
    let caret = app.workspace.editor().text.line_to_byte(2000);
    app.workspace
        .editor_mut()
        .set_selection(Selection::from_regions([Region::caret(caret)]));
    app.changed_state(Instant::now(), true);
    assert_eq!(app.view.line_height, 15.5);
    let rect = app.view.rect_at(app.workspace.editor(), caret);
    assert!(rect.y >= app.view.tab_height);
    assert!(rect.y + rect.height <= app.view.size.height);
    assert_eq!(app.panels.output_view.line_height, 15.5);
    // Output and find results share this output view and the same metric snapshot.
}
#[test]
fn releasing_selected_text_before_drag_delay_places_the_caret() {
    let mut app = app("one two", None);
    app.mouse_preferences.text_drag_delay = vel_platform::mouse::MousePreferences::text_delay(300.);
    app.workspace
        .editor_mut()
        .set_selection(Selection::from_regions([Region::new(0, 3)]));
    let rect = app.view.rect_at(app.workspace.editor(), 1);
    let point = Point {
        x: rect.x,
        y: rect.y + 5.,
    };
    mouse(&mut app, point, MouseKind::Down);
    mouse(&mut app, point, MouseKind::Up);
    assert_eq!(app.workspace.editor().caret, 1);
    assert!(app.workspace.editor().selection().is_empty());
    assert_eq!(app.workspace.editor().text.to_string(), "one two");
}
#[test]
#[ignore = "explicit ragged-selection scene budget at 50, 200 and 1000 visible rows"]
fn ragged_selection_scene_budget_50_200_1000_visible_lines() {
    for rows in [50, 200, 1000] {
        let source = (0..rows)
            .map(|i| format!("{}\n", "x".repeat(5 + (i * 73) % 137)))
            .collect::<String>();
        let mut app = app(&source, None);
        app.view.size = Size {
            width: 1400.,
            height: rows as f64 * 20. + 38.,
        };
        app.workspace.wait_resources();
        let measure = |app: &mut App| {
            let start = Instant::now();
            for _ in 0..100 {
                std::hint::black_box(app.view.scene(app.workspace.editor()));
            }
            start.elapsed().as_secs_f64() * 1e6 / 100.
        };
        app.view.scene(app.workspace.editor());
        let plain = measure(&mut app);
        named(&mut app, "select_all", json!({}));
        app.view.scroll_y = 0.;
        let start = Instant::now();
        let scene = app.view.scene(app.workspace.editor());
        let cold = start.elapsed().as_secs_f64() * 1e6;
        let cached = measure(&mut app);
        let contour = &scene.layers[0].contours[0];
        println!(
            "RAGGED_SELECTION rows={rows} points={} plain_us={plain:.3} cold_us={cold:.3} cached_us={cached:.3}",
            contour.points.len()
        );
        // A linear contour must stay a small part of the real scene, even on ragged
        // code. Allow scheduling noise while bounding the previously multi-ms cost.
        assert!(
            cold < plain * 2. + 1000.,
            "rows={rows}: cold={cold} plain={plain}"
        );
        assert!(
            cached < plain * 1.5 + 100.,
            "rows={rows}: cached={cached} plain={plain}"
        );
        assert!(contour.points.len() <= rows * 20 + 20);
    }
}

#[test]
fn minimap_border_draws_four_viewport_edges_without_hover_through_dispatch() {
    let mut app = app(&"line\n".repeat(500), None);
    app.view.minimap_hover = false;
    named(
        &mut app,
        "set_setting",
        json!({"setting":"draw_minimap_border","value":false}),
    );
    // Wait for the actual highlighting/minimap publication, rather than comparing
    // a scene before sampling with one after the worker's first result arrives.
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        app.workspace.poll();
        let language = app.workspace.editor().language.borrow();
        if language.as_ref().is_none_or(|language| !language.busy()) {
            assert!(
                language
                    .as_ref()
                    .is_none_or(|language| !language.minimap.is_empty())
            );
            break;
        }
        assert!(Instant::now() < deadline, "minimap samples did not settle");
        std::thread::park_timeout(Duration::from_millis(1));
    }
    let before = app.view.scene(app.workspace.editor());
    named(
        &mut app,
        "set_setting",
        json!({"setting":"draw_minimap_border","value":true}),
    );
    let after = app.view.scene(app.workspace.editor());
    assert_eq!(
        after.layers.iter().map(|l| l.rects.len()).sum::<usize>(),
        before.layers.iter().map(|l| l.rects.len()).sum::<usize>() + 4
    );
    assert!(!app.view.minimap_hover);
    assert!(
        !app.workspace
            .editor()
            .settings
            .bool("always_show_minimap_viewport", true)
    );
}

#[test]
fn minimap_release_over_status_bar_ends_capture_before_wheel_and_text_drag() {
    let mut app = app(&"long line of text\n".repeat(10_000), None);
    let (bounds, viewport) = app.view.minimap_rects(app.workspace.editor());
    let press = Point {
        x: bounds.x + 20.,
        y: viewport.y + viewport.height / 2.,
    };
    let caret = app.workspace.editor().caret;
    mouse(&mut app, press, MouseKind::Down);
    let status = Point {
        y: app.window_size.height - 1. - app.content.y,
        ..press
    };
    mouse(&mut app, status, MouseKind::Drag);
    assert!(app.view.scroll_y > 1000. * app.view.line_height);
    assert_eq!(app.workspace.editor().caret, caret);
    mouse(&mut app, status, MouseKind::Up);
    assert!(!app.view.pointer_gesture_active());
    assert!(app.drag_select.is_none() && app.drag_text.is_none());
    let before = app.view.scroll_y;
    app.handle_scroll_event(
        None,
        ScrollEvent {
            position: Point {
                x: app.content.x + 200.,
                y: app.content.y + 100.,
            },
            delta_x: 0.,
            delta_y: 5.,
            precise: false,
            phase: 0,
            momentum_phase: 0,
            modifiers: Modifiers(0),
            timestamp: 0.,
        },
    );
    assert!(app.view.scroll_y < before && app.view.scroll_y > 1000. * app.view.line_height);
    let (bounds, viewport) = app.view.minimap_rects(app.workspace.editor());
    assert!(
        viewport.y >= bounds.y && viewport.y + viewport.height <= bounds.y + bounds.height + 1e-6
    );
    let next = Point {
        x: bounds.x + 20.,
        y: bounds.y + 100.,
    };
    mouse(&mut app, next, MouseKind::Down);
    assert!(app.view.minimap_drag);
    let stable = app.view.scroll_y;
    mouse(&mut app, next, MouseKind::Drag);
    assert!((app.view.scroll_y - stable).abs() < 1e-6);
    mouse(&mut app, next, MouseKind::Up);
    let text = Point {
        x: app.view.gutter(app.workspace.editor()) + 10.,
        y: app.view.tab_height + 12.,
    };
    mouse(&mut app, text, MouseKind::Down);
    mouse(
        &mut app,
        Point {
            x: text.x + 40.,
            ..text
        },
        MouseKind::Drag,
    );
    assert!(
        !app.workspace
            .editor()
            .buffer
            .view(app.workspace.editor().id)
            .sel()
            .primary()
            .unwrap()
            .empty()
    );
    assert!(!app.view.minimap_drag);
    mouse(&mut app, status, MouseKind::Up);
    assert!(app.drag_select.is_none() && app.drag_text.is_none());
    assert!(app.view.drop_caret.is_none());
    // A new press also cancels an interrupted gesture whose up never arrived.
    mouse(&mut app, next, MouseKind::Down);
    assert!(app.view.minimap_drag);
    mouse(&mut app, text, MouseKind::Down);
    assert!(!app.view.pointer_gesture_active());
    mouse(
        &mut app,
        Point {
            x: text.x + 40.,
            ..text
        },
        MouseKind::Drag,
    );
    assert!(app.drag_select.is_some() || app.drag_text.is_some());
    mouse(&mut app, status, MouseKind::Up);
    assert!(app.view.drop_caret.is_none());
}

#[test]
fn repeated_zoom_with_polling_preserves_every_press_and_then_applies_external_preferences() {
    use std::time::Duration;
    for interval in [8, 33] {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app("", Some(dir.path()));
        for press in 1..=10 {
            key(&mut app, "super+=");
            if press == 3 || press == 7 {
                app.panels
                    .review_action(
                        &mut app.workspace,
                        "persist_spelling_words",
                        &json!({
                            "key":"added_words","words":[format!("word{press}")]
                        }),
                    )
                    .unwrap();
            }
            let until = Instant::now() + Duration::from_millis(interval);
            while Instant::now() < until {
                app.workspace.poll();
                app.sync_active_view();
                assert_eq!(
                    app.workspace.editor().preferences.font_size,
                    13. + press as f64,
                    "interval={interval} press={press}"
                );
                std::thread::park_timeout(Duration::from_millis(1));
            }
        }
        app.workspace.wait_resources();
        let path = dir
            .path()
            .join("Packages/User/Preferences.sublime-settings");
        let source = std::fs::read_to_string(&path).unwrap();
        let document = vel_config::json::Document::parse(&source).unwrap();
        assert_eq!(document.value["font_size"], 23.);
        assert_eq!(app.workspace.editor().preferences.font_size, 23.);
        assert_eq!(document.value["added_words"], json!(["word7"]));
        assert_eq!(
            app.workspace.settings.get("added_words"),
            Some(&json!(["word7"]))
        );
        std::fs::write(&path, document.set("font_size", &json!(31.)).unwrap()).unwrap();
        app.workspace.wait_resources();
        assert_eq!(app.workspace.editor().preferences.font_size, 31.);
        assert!(
            app.workspace.messages.is_empty(),
            "{:?}",
            app.workspace.messages
        );
    }
}

#[test]
fn legacy_session_pixels_use_the_restored_font_size_and_padding() {
    for (size, top, bottom) in [(16., 0., 0.), (13., 4., 0.), (16., 4., 3.)] {
        let dir = tempfile::tempdir().unwrap();
        let user = dir.path().join("Packages/User");
        std::fs::create_dir_all(&user).unwrap();
        std::fs::write(
            user.join("Preferences.sublime-settings"),
            json!({
                "font_size":size,"line_padding_top":top,"line_padding_bottom":bottom
            })
            .to_string(),
        )
        .unwrap();
        let mut source = app(&"line\n".repeat(3000), None);
        let (window, documents) = source.workspace.snapshot().unwrap();
        let mut wire = serde_json::to_value(&window).unwrap();
        wire["sheets"][0]["view"]["scroll"] =
            json!([0., 1000.25 * (20. * size / 13. + top + bottom)]);
        wire["sheets"][0]["view"]
            .as_object_mut()
            .unwrap()
            .remove("scroll_line");
        let window = serde_json::from_value(wire).unwrap();
        let mut restored = app("", Some(dir.path()));
        restored
            .workspace
            .restore_window(&window, &documents)
            .unwrap();
        restored.view_buffer = None;
        install_test_metrics(&mut restored, 2.);
        restored.sync_active_view();
        assert!((restored.view.scroll_position().visual_row - 1000.25).abs() < 1e-6);
        // The current wire field overrides the compatibility pixel field exactly once.
        let tab = &mut restored.workspace.tabs[restored.workspace.active];
        tab.scroll = (restored.view.scroll_x, restored.view.scroll_y);
        tab.scroll_line_height = restored.view.line_height;
        let (current, documents) = restored.workspace.snapshot().unwrap();
        restored
            .workspace
            .restore_window(&current, &documents)
            .unwrap();
        restored.view_buffer = None;
        restored.sync_active_view();
        assert!((restored.view.scroll_position().visual_row - 1000.25).abs() < 1e-6);
    }
}

#[test]
fn restored_scroll_clamps_when_the_saved_file_shrank() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("shrinking.txt");
    std::fs::write(&path, "line\n".repeat(3000)).unwrap();
    let mut source = app("", Some(dir.path()));
    source
        .workspace
        .open(FileLocation {
            path: path.clone(),
            line: 1,
            column: 1,
        })
        .unwrap();
    source.sync_active_view();
    source.view.scroll_y = 2500. * source.view.line_height;
    let tab = &mut source.workspace.tabs[source.workspace.active];
    tab.scroll = (0., source.view.scroll_y);
    tab.scroll_line_height = source.view.line_height;
    let (window, documents) = source.workspace.snapshot().unwrap();
    std::fs::write(&path, "line\n".repeat(100)).unwrap();
    let mut restored = app("", Some(dir.path()));
    restored
        .workspace
        .restore_window(&window, &documents)
        .unwrap();
    restored.view_buffer = None;
    install_test_metrics(&mut restored, 1.);
    restored.sync_active_view();
    assert_eq!(restored.workspace.editor().text.len_lines(), 101);
    assert!(restored.view.scroll_position().visual_row <= 100.);
    let last = restored.view.rect_at(
        restored.workspace.editor(),
        restored.workspace.editor().text.len_bytes(),
    );
    assert!(last.y >= restored.view.tab_height && last.y < restored.view.size.height);
}

#[test]
fn visual_row_scroll_api_restores_through_zoom_and_scale_changes() {
    let mut app = app(&"line\n".repeat(3000), None);
    install_test_metrics(&mut app, 1.);
    app.view.scroll_y = 1000.25 * app.view.line_height;
    let saved = app.view.scroll_position();
    key(&mut app, "super+=");
    app.workspace.themes.ui.borrow_mut().density = 2.;
    app.sync_active_view();
    app.view.scroll_y = 0.;
    app.view
        .restore_scroll_position(app.workspace.editor(), saved);
    assert_eq!(app.view.scroll_position(), saved);
}

#[test]
fn restored_scroll_waits_for_background_file_loading_before_clamping() {
    for shrank in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("background.txt");
        std::fs::write(&path, "line\n".repeat(3000)).unwrap();
        let mut source = app("", None);
        source
            .workspace
            .open(FileLocation {
                path: path.clone(),
                line: 1,
                column: 1,
            })
            .unwrap();
        source.sync_active_view();
        let tab = &mut source.workspace.tabs[source.workspace.active];
        tab.scroll = (0., 2500.25 * source.view.line_height);
        tab.scroll_line_height = source.view.line_height;
        let (window, mut documents) = source.workspace.snapshot().unwrap();
        // Legacy sessions without native history use the disk loading path.
        for document in &mut documents {
            document.history = None;
        }
        if shrank {
            std::fs::write(&path, "line\n".repeat(100)).unwrap();
        }
        let user = dir.path().join("Packages/User");
        std::fs::create_dir_all(&user).unwrap();
        std::fs::write(
            user.join("Preferences.sublime-settings"),
            "{\"large_file.load_bytes\":1}",
        )
        .unwrap();
        let mut restored = app("", Some(dir.path()));
        restored
            .workspace
            .restore_window(&window, &documents)
            .unwrap();
        assert!(
            restored.workspace.tabs[restored.workspace.active]
                .loading
                .is_some()
        );
        restored.view_buffer = None;
        install_test_metrics(&mut restored, 1.);
        restored.sync_active_view();
        assert!((restored.view.scroll_position().visual_row - 2500.25).abs() < 1e-6);
        let deadline = Instant::now() + Duration::from_secs(10);
        while restored.workspace.tabs[restored.workspace.active]
            .loading
            .is_some()
        {
            assert!(Instant::now() < deadline);
            restored.workspace.poll();
            restored.sync_active_view();
            std::thread::yield_now();
        }
        if shrank {
            assert_eq!(restored.workspace.editor().text.len_lines(), 101);
            assert!(restored.view.scroll_position().visual_row <= 100.);
        } else {
            assert!((restored.view.scroll_position().visual_row - 2500.25).abs() < 1e-6);
        }
    }
}

#[test]
fn legacy_lazy_session_normalizes_scroll_before_zoom_and_resave() {
    let dir = tempfile::tempdir().unwrap();
    let user = dir.path().join("Packages/User");
    std::fs::create_dir_all(&user).unwrap();
    std::fs::write(
        user.join("Preferences.sublime-settings"),
        "{\"font_size\":16,\"line_padding_top\":4}",
    )
    .unwrap();
    let mut source = app(&"line\n".repeat(3000), Some(dir.path()));
    named(&mut source, "new_file", json!({}));
    named(
        &mut source,
        "insert",
        json!({"characters":"line\n".repeat(3000)}),
    );
    let (window, documents) = source.workspace.snapshot().unwrap();
    let mut wire = serde_json::to_value(window).unwrap();
    for sheet in wire["sheets"].as_array_mut().unwrap() {
        sheet["view"]["scroll"] = json!([0., 1000.25 * (20. * 16. / 13. + 4.)]);
        sheet["view"].as_object_mut().unwrap().remove("scroll_line");
    }
    let mut restored = app("", Some(dir.path()));
    restored
        .workspace
        .restore_window(&serde_json::from_value(wire).unwrap(), &documents)
        .unwrap();
    let lazy = restored
        .workspace
        .tabs
        .iter()
        .position(|tab| tab.lazy.is_some())
        .unwrap();
    restored.view_buffer = None;
    restored.sync_active_view();
    key(&mut restored, "super+=");
    restored.workspace.wait_resources();
    assert_eq!(restored.workspace.editor().preferences.font_size, 17.);
    assert!(restored.workspace.tabs[lazy].lazy.is_some());
    let (saved, documents) = restored.workspace.snapshot().unwrap();
    assert!((saved.sheets[lazy].view.scroll_line.unwrap() - 1000.25).abs() < 1e-6);
    restored
        .workspace
        .restore_window(&saved, &documents)
        .unwrap();
    restored.workspace.activate_sheet(lazy);
    restored.view_buffer = None;
    install_test_metrics(&mut restored, 2.);
    restored.sync_active_view();
    assert!((restored.view.scroll_position().visual_row - 1000.25).abs() < 1e-6);
}

#[test]
fn minimap_clicks_centre_the_drawn_row_before_starting_a_continuous_drag() {
    let mut app = app(
        &"a long line wrapping over several rows\n".repeat(10_000),
        None,
    );
    install_test_metrics(&mut app, 1.);
    app.view.scene(app.workspace.editor());
    for scroll_fraction in [0., 0.5, 1.] {
        for click_fraction in [0.02, 0.18, 0.36, 0.72, 0.97] {
            app.view.scroll(app.workspace.editor(), 0., 1e9);
            let max = app.view.scroll_y;
            app.view.scroll_y = max * scroll_fraction;
            let (bounds, viewport) = app.view.minimap_rects(app.workspace.editor());
            let point = Point {
                x: bounds.x + 20.,
                y: bounds.y + bounds.height * click_fraction,
            };
            if vel_ui::contains(viewport, point) {
                continue;
            }
            let drawn = app.view.minimap_visual_at(app.workspace.editor(), point.y);
            let visible = app.view.size.height - app.view.tab_height - 8.;
            mouse(&mut app, point, MouseKind::Down);
            let centre = (app.view.scroll_y + visible / 2.) / app.view.line_height;
            assert!(
                (centre - drawn).abs() <= 1.,
                "scroll={scroll_fraction} y={} drawn={drawn} centre={centre}",
                point.y
            );
            let after_click = app.view.scroll_y;
            for _ in 0..10 {
                mouse(&mut app, point, MouseKind::Drag);
                assert!((app.view.scroll_y - after_click).abs() < 1e-6);
            }
            let movement = if click_fraction < 0.9 { 1. } else { -1. };
            mouse(
                &mut app,
                Point {
                    y: point.y + movement,
                    ..point
                },
                MouseKind::Drag,
            );
            let expected = if movement > 0. {
                after_click + (max - after_click) / (bounds.y + bounds.height - point.y)
            } else {
                after_click - after_click / (point.y - bounds.y)
            };
            assert!((app.view.scroll_y - expected).abs() < 1e-6);
            let (_, before_release) = app.view.minimap_rects(app.workspace.editor());
            mouse(
                &mut app,
                Point {
                    y: point.y + movement,
                    ..point
                },
                MouseKind::Up,
            );
            assert_eq!(
                app.view.minimap_rects(app.workspace.editor()).1,
                before_release
            );
        }
    }
}

#[test]
fn minimap_outside_click_drags_reach_both_ends_from_top_middle_and_bottom() {
    for (wrap, folded) in [(false, false), (true, false), (true, true)] {
        for lines in [300, 10_000] {
            let dir = tempfile::tempdir().unwrap();
            let user = dir.path().join("Packages/User");
            std::fs::create_dir_all(&user).unwrap();
            std::fs::write(
                user.join("Preferences.sublime-settings"),
                json!({"word_wrap":wrap,"wrap_width":12,"show_minimap":true,
                    "scroll_past_end":true,"overlay_scroll_bars":"disabled"})
                .to_string(),
            )
            .unwrap();
            let mut app = app(
                &"a long line wrapping over several rows\n".repeat(lines),
                Some(dir.path()),
            );
            install_test_metrics(&mut app, 1.);
            if folded {
                let e = app.workspace.editor_mut();
                e.set_folded(vec![Region::new(
                    e.text.line_to_byte(lines / 10),
                    e.text.line_to_byte(lines / 2),
                )]);
                assert!(!e.folded().is_empty());
            }
            app.view.scene(app.workspace.editor());
            app.view.scroll(app.workspace.editor(), 0., 1e9);
            let max = app.view.scroll_y;
            assert!(max > app.view.size.height);
            for press_fraction in [0.02, 0.5, 0.98] {
                // Put the box at the opposite end, so every press really is outside.
                app.view.scroll_y = if press_fraction < 0.5 { max } else { 0. };
                let (bounds, viewport) = app.view.minimap_rects(app.workspace.editor());
                let press = Point {
                    x: bounds.x + bounds.width / 2.,
                    y: bounds.y + press_fraction * bounds.height,
                };
                assert!(!vel_ui::contains(viewport, press));
                mouse(&mut app, press, MouseKind::Down);
                let anchor = app.view.scroll_y;
                assert!((0. ..=max).contains(&anchor));
                for direction in [-1., 1.] {
                    mouse(&mut app, press, MouseKind::Drag);
                    assert!((app.view.scroll_y - anchor).abs() < 1e-6);
                    let end_y = if direction < 0. {
                        bounds.y
                    } else {
                        bounds.y + bounds.height
                    };
                    let span = (end_y - press.y).abs();
                    let end_scroll = if direction < 0. { 0. } else { max };
                    let mut previous = anchor;
                    for distance in [1., span / 4., span / 2., span] {
                        let point = Point {
                            y: press.y + direction * distance,
                            ..press
                        };
                        mouse(&mut app, point, MouseKind::Drag);
                        let expected = anchor + (end_scroll - anchor) * distance / span;
                        assert!(
                            (app.view.scroll_y - expected).abs() < 1e-6,
                            "wrap={wrap} folded={folded} lines={lines} press={press_fraction} distance={distance}"
                        );
                        assert!((app.view.scroll_y - previous) * direction >= -1e-6);
                        previous = app.view.scroll_y;
                        for _ in 0..3 {
                            mouse(&mut app, point, MouseKind::Drag);
                            assert!((app.view.scroll_y - expected).abs() < 1e-6);
                        }
                    }
                    assert!((app.view.scroll_y - end_scroll).abs() < 1e-6);
                }
                let bottom = Point {
                    y: bounds.y + bounds.height,
                    ..press
                };
                mouse(&mut app, bottom, MouseKind::Up);
                assert!(!app.view.minimap_drag);
                assert!((app.view.scroll_y - max).abs() < 1e-6);
            }
        }
    }
}

#[test]
fn blocked_preference_queue_defers_one_reload_and_tracks_multiple_keys_per_write() {
    struct ReleaseQueue(Option<std::sync::mpsc::Sender<()>>);
    impl Drop for ReleaseQueue {
        fn drop(&mut self) {
            if let Some(tx) = self.0.take() {
                let _ = tx.send(());
            }
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let user = dir.path().join("Packages/User");
    std::fs::create_dir_all(&user).unwrap();
    std::fs::write(
        user.join("Preferences.sublime-settings"),
        "{\"font_size\":13,\"added_words\":[\"old\"]}",
    )
    .unwrap();
    let mut app = app("", Some(dir.path()));
    for _ in 0..10 {
        named(&mut app, "new_file", json!({}));
    }
    let (tx, rx) = std::sync::mpsc::channel();
    let (started, ready) = std::sync::mpsc::channel();
    let blocker = crate::release::enqueue(move || {
        started.send(()).map_err(|e| e.to_string())?;
        rx.recv_timeout(Duration::from_secs(120))
            .map_err(|e| e.to_string())
    })
    .unwrap();
    let mut release = ReleaseQueue(Some(tx));
    ready.recv_timeout(Duration::from_secs(120)).unwrap();
    key(&mut app, "super+=");
    key(&mut app, "super+=");
    let updates = vec![
        ("added_words".to_owned(), Some(json!(["new"]))),
        ("ignored_words".to_owned(), Some(json!(["ignore"]))),
        ("rulers".to_owned(), Some(json!([79]))),
    ];
    for (key, value) in &updates {
        app.workspace.settings.set(
            vel_config::settings::Layer::User,
            key,
            value.clone().unwrap(),
        );
        for tab in &mut app.workspace.tabs {
            tab.editor.settings.set(
                vel_config::settings::Layer::User,
                key,
                value.clone().unwrap(),
            );
            tab.editor.sync();
        }
    }
    let path = user.join("Preferences.sublime-settings");
    let written_updates = updates.clone();
    let write = crate::release::enqueue(move || {
        let mut text = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
        for (key, value) in written_updates {
            text = vel_config::json::Document::parse(&text)?.set(&key, &value.unwrap())?;
        }
        vel_session::atomic_write(&path, |file| {
            use std::io::Write;
            file.write_all(text.as_bytes()).map_err(|e| e.to_string())
        })
    })
    .unwrap();
    app.workspace.track_settings_write(write, &updates);
    std::fs::write(user.join("Plain Text.sublime-settings"), "{\"tab_size\":7}").unwrap();
    let revision = app.workspace.resource_revision;
    // Complete the external resource load without waiting for the blocked queue.
    // Repeated explicit disk probes exercise deferral without a three-second race
    // against the production 500 ms configuration check.
    app.workspace.poll_resource_reload();
    let pending_revision = app.workspace.resource_revision;
    assert_eq!(pending_revision, revision + 1);
    for _ in 0..10 {
        app.workspace.poll_resource_reload();
        assert_eq!(
            app.workspace.resource_revision, pending_revision,
            "reload spin"
        );
        assert_eq!(app.workspace.editor().preferences.font_size, 15.);
        for (key, value) in &updates {
            assert_eq!(app.workspace.settings.get(key), value.as_ref());
        }
    }
    assert_eq!(app.workspace.editor().preferences.tab_size, 7);
    // No poll races an atomic write or its acknowledgement: this serial fence
    // runs only after both zoom jobs and the multi-key write have returned.
    let drained = crate::release::enqueue(|| Ok(())).unwrap();
    release.0.take().unwrap().send(()).unwrap();
    blocker
        .recv_timeout(Duration::from_secs(120))
        .unwrap()
        .unwrap();
    drained
        .recv_timeout(Duration::from_secs(120))
        .unwrap()
        .unwrap();
    app.workspace.wait_resources();
    assert_eq!(app.workspace.resource_revision, pending_revision + 1);
    // Our completed writes are already in the settled snapshot. Later automatic
    // checks must not rediscover them as external edits and request another load.
    for _ in 0..10 {
        app.workspace.poll_resource_reload();
        assert_eq!(app.workspace.resource_revision, pending_revision + 1);
    }
    assert!(
        app.workspace
            .tabs
            .iter()
            .all(|tab| tab.editor.preferences.font_size == 15.)
    );
    let document = vel_config::json::Document::parse(
        &std::fs::read_to_string(user.join("Preferences.sublime-settings")).unwrap(),
    )
    .unwrap();
    assert_eq!(document.value["font_size"], 15.);
    for (key, value) in updates {
        assert_eq!(app.workspace.settings.get(&key), value.as_ref());
        assert_eq!(document.value[&key], value.unwrap());
    }
    assert_eq!(app.workspace.editor().preferences.tab_size, 7);
    assert!(
        app.workspace.messages.is_empty(),
        "{:?}",
        app.workspace.messages
    );
}

#[test]
fn fixed_user_json_reloads_after_an_in_place_repair() {
    let dir = tempfile::tempdir().unwrap();
    let user = dir.path().join("Packages/User");
    std::fs::create_dir_all(&user).unwrap();
    let path = user.join("Preferences.sublime-settings");
    std::fs::write(&path, "{\"font_size\":19}").unwrap();
    let mut app = app("", Some(dir.path()));
    std::fs::write(&path, "{broken user JSON").unwrap();
    app.workspace.wait_resources();
    assert!(app.workspace.messages.is_empty());
    assert!(
        app.workspace
            .resource_reports
            .iter()
            .any(|m| m.contains("Preferences.sublime-settings") && m.contains("line 1"))
    );
    assert_eq!(app.workspace.editor().preferences.font_size, 19.);
    let revision = app.workspace.resource_revision;
    let parent_modified = std::fs::metadata(&user).unwrap().modified().unwrap();
    std::fs::write(&path, "{\"font_size\":20}").unwrap();
    assert_eq!(
        std::fs::metadata(&user).unwrap().modified().unwrap(),
        parent_modified
    );
    app.workspace.wait_resources();
    assert!(app.workspace.resource_revision > revision);
    assert_eq!(app.workspace.editor().preferences.font_size, 20.);
}

#[test]
fn status_bar_syntax_picker_preserves_the_scrolled_view_on_press_and_release() {
    let mut app = app(&"line\n".repeat(10_000), None);
    app.view.scroll_y = 5000. * app.view.line_height;
    // Status items are hit where the last frame painted them.
    app.scene();
    let position = app.view.scroll_position();
    let item = app
        .panels
        .status_item(crate::panels::StatusItem::Syntax)
        .expect("syntax item painted");
    let status = Point {
        x: item.x + item.width / 2. - app.content.x,
        y: app.window_size.height - 1. - app.content.y,
    };
    mouse(&mut app, status, MouseKind::Down);
    assert!(app.panels.overlay.is_some());
    assert_eq!(app.view.scroll_position(), position);
    mouse(&mut app, status, MouseKind::Up);
    assert_eq!(app.view.scroll_position(), position);
}
