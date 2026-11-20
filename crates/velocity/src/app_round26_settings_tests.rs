use super::*;
use std::path::Path;
use vel_config::settings::Layer as SettingsLayer;
use vel_render::scene::Surface;
use vel_text::Text;

fn app(data: Option<&Path>) -> App {
    let workspace = Workspace::new(
        Text::from("line\n".repeat(80).as_str()),
        data.map(Path::to_owned),
        None,
        false,
    )
    .unwrap();
    let options = Options {
        mode: Mode::Interactive,
        ..Options::default()
    };
    App::new(
        options.clone(),
        Clock::new(Instant::now(), std::time::SystemTime::now()),
        Reporter::for_options(&options, Instant::now()).unwrap(),
        workspace,
        Arc::new(AtomicBool::new(false)),
        std::rc::Rc::new(std::cell::RefCell::new(crate::session::Sessions::new(None))),
    )
}

fn preference(app: &mut App, key: &str, value: Value) {
    let e = app.workspace.editor_mut();
    e.settings.set(SettingsLayer::View, key, value);
    e.sync();
}

fn user_file(data: &Path, contents: &Value) -> PathBuf {
    let path = data.join("Packages/User/Preferences.sublime-settings");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, contents.to_string()).unwrap();
    path
}

#[test]
fn rounded_and_square_tab_styles_change_the_drawn_tab_not_the_hit_rect() {
    let mut app = app(None);
    for dark in [false, true] {
        app.workspace.set_appearance(dark).unwrap();
        let mut original_hit = None;
        for (style, radius) in [
            ("square", 0.),
            ("rounded", 6.),
            ("angled", 0.),
            ("other", 0.),
        ] {
            preference(&mut app, "file_tab_style", json!(style));
            let scene = app.scene();
            let tab = app.tab_hits[0].1;
            assert_eq!(tab, *original_hit.get_or_insert(tab));
            assert!(scene.layers.iter().flat_map(|l| &l.surfaces).any(|s| {
                matches!(s, Surface::Fill(q) if q.rect == tab && (q.radius - radius).abs() < 0.01)
            }), "{dark} {style} {tab:?}");
        }
    }
}

#[test]
fn tab_close_side_moves_target_and_painted_label_without_moving_the_tab() {
    let mut app = app(None);
    app.workspace.tabs[0].suggested_name = Some("Test Tab".into());
    app.panels.side_visible = false;
    let mut original_hit = None;
    let mut original_label_x = None;
    for left in [false, true] {
        preference(&mut app, "show_tab_close_buttons_on_left", json!(left));
        let scene = app.scene();
        let tab = app.tab_hits[0].1;
        assert_eq!(tab, *original_hit.get_or_insert(tab));
        let (sheet, close) = app.tab_close_hits[0];
        assert_eq!(sheet, app.tab_hits[0].0);
        let label = scene
            .layers
            .iter()
            .flat_map(|layer| &layer.text)
            .find(|text| {
                text.text == "Test Tab"
                    && f64::from(text.x) >= tab.x
                    && f64::from(text.x) < tab.x + tab.width
                    && f64::from(text.baseline) < tab.y + tab.height
            })
            .expect("selected tab label");
        let middle = tab.x + tab.width / 2.;
        if left {
            assert!(close.x + close.width < middle);
            assert!(f64::from(label.x) > close.x + close.width);
            assert!(label.x > original_label_x.unwrap());
        } else {
            assert!(close.x > middle);
            assert!(f64::from(label.x) < close.x);
            original_label_x = Some(label.x);
        }
        assert!(
            scene.layers.iter().flat_map(|l| &l.surfaces).any(|s| {
                matches!(s, Surface::Image(q)
                if q.rect.x >= close.x && q.rect.x + q.rect.width <= close.x + close.width
                    && q.rect.y >= close.y && q.rect.y + q.rect.height <= close.y + close.height)
            }),
            "close glyph painted inside the target"
        );
    }
    preference(&mut app, "show_tab_close_buttons", json!(false));
    app.scene();
    assert!(app.tab_close_hits.is_empty());
}

#[test]
fn scrollbar_setting_overrides_system_and_updates_geometry_live() {
    let mut app = app(None);
    preference(&mut app, "show_minimap", json!(false));
    for (setting, os_overlay, overlay) in [
        ("system", false, false),
        ("system", true, true),
        ("enabled", false, true),
        ("disabled", true, false),
    ] {
        preference(&mut app, "overlay_scroll_bars", json!(setting));
        app.view.set_system_scroller_overlay(os_overlay);
        app.scene();
        let right = app.view.right(app.workspace.editor());
        assert_eq!(app.view.size.width - right, if overlay { 0. } else { 8. });
        let (track, _) = app.view.scrollbar_rects();
        if overlay {
            assert_eq!(track, Rect::default(), "overlay track hides while idle");
            app.view.scroll(app.workspace.editor(), 0., 20.);
            app.scene();
            assert_eq!(app.view.scrollbar_rects().0.x + 8., right);
        } else {
            assert_eq!(track.x, right);
            assert_eq!(track.width, 8.);
        }
        // Independent runs exercise the idle policy rather than a preceding scroll.
        app.view = View::new(app.view.size, 8., 12.);
    }
}

#[test]
fn new_window_settings_are_a_validated_snapshot_not_a_global_edit() {
    let data = tempfile::tempdir().unwrap();
    let data = data.path();
    user_file(
        data,
        &json!({"font_size":14, "new_window_settings":{"font_size":21,"show_minimap":false,"wrap_width":"invalid"}}),
    );
    let ordinary = Workspace::new(Text::new(), Some(data.into()), None, false).unwrap();
    assert_eq!(ordinary.editor().preferences.font_size, 14.);
    assert!(ordinary.window_visible("show_minimap"));
    let mut new_window = Workspace::new_window(Some(data.into()), false).unwrap();
    assert_eq!(new_window.editor().preferences.font_size, 21.);
    assert!(!new_window.window_visible("show_minimap"));
    assert_eq!(new_window.settings.usize("wrap_width", 999), 0);
    assert!(
        new_window
            .take_resource_reports()
            .iter()
            .any(|s| s.contains("new_window_settings: wrap_width"))
    );
    user_file(
        data,
        &json!({"line_numbers":false, "new_window_settings":{"font_size":18}}),
    );
    new_window.request_resource_reload();
    new_window.wait_resources();
    assert_eq!(new_window.editor().preferences.font_size, 21.);
    assert!(!new_window.editor().preferences.line_numbers);
    new_window.new_file().unwrap();
    assert_eq!(new_window.editor().preferences.font_size, 21.);
    let next_window = Workspace::new_window(Some(data.into()), false).unwrap();
    assert_eq!(next_window.editor().preferences.font_size, 18.);
    assert!(next_window.window_visible("show_minimap"));
}

#[test]
fn settings_user_tab_save_updates_existing_window_and_preserves_explicit_toggles() {
    let data = tempfile::tempdir().unwrap();
    let path = user_file(data.path(), &json!({"font_size":13}));
    let mut app = app(Some(data.path()));
    assert!(app.panels.side_visible && app.panels.chrome);
    assert!(
        app.apply_settings_action(
            None,
            "edit_settings",
            &json!({
                "base_file":"Packages/Default/Preferences.sublime-settings"
            })
        )
        .unwrap()
    );
    let mut child = app.opened_settings.pop().unwrap().workspace;
    assert_eq!(
        child.tabs[child.active].path.as_deref(),
        Some(path.canonicalize().unwrap().as_path())
    );
    assert!(child.tabs.iter().any(|tab| tab.editor.read_only));
    child.editor_mut().run("select_all", &json!({})).unwrap();
    child
        .editor_mut()
        .run(
            "insert",
            &json!({"characters":json!({
        "font_size":19, "show_side_bar":false, "show_status_bar":false, "show_tabs":false,
        "show_minimap":false, "overlay_scroll_bars":"disabled", "file_tab_style":"rounded"
    }).to_string()}),
        )
        .unwrap();
    child.save(child.active, None).unwrap();
    let until = Instant::now() + Duration::from_secs(10);
    while app.workspace.editor().preferences.font_size != 19. {
        app.poll_workspace_model();
        assert!(Instant::now() < until, "{:?}", app.workspace.messages);
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(!app.panels.side_visible && !app.panels.chrome);
    assert!(!app.workspace.window_visible("show_tabs"));
    assert!(!app.workspace.window_visible("show_minimap"));
    app.scene();
    assert!(app.tab_hits.is_empty());
    assert_eq!(app.view.scrollbar_rects().0.width, 8.);

    app.workspace
        .set_window_toggle("show_side_bar", false)
        .unwrap();
    user_file(
        data.path(),
        &json!({"show_side_bar":true, "show_status_bar":true}),
    );
    let until = Instant::now() + Duration::from_secs(10);
    while !app.panels.chrome {
        app.poll_workspace_model();
        assert!(Instant::now() < until, "{:?}", app.workspace.messages);
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(!app.panels.side_visible, "window toggle wins over User");
}

#[cfg(target_os = "macos")]
#[test]
fn output_view_uses_the_measured_wide_advances() {
    let mut app = app(None);
    let mut fonts = vel_render::font::Fonts::load().unwrap();
    let advances = fonts.wide_advances();
    assert_ne!(advances, Default::default());
    app.fonts = Some(fonts);
    app.initialize_view_metrics();
    assert_eq!(app.view.wide_advances, advances);
    assert_eq!(app.panels.output_view.wide_advances, advances);
}
