use super::*;
use std::{cell::RefCell, rc::Rc};
use vel_config::keymap::Key;
use vel_text::Text;

type Shared = Rc<RefCell<crate::session::Sessions>>;
fn sessions() -> Shared {
    Rc::new(RefCell::new(crate::session::Sessions::new(None)))
}
fn app(text: &str, project: Option<PathBuf>, sessions: Shared) -> App {
    app_with_profile(text, project, sessions, None)
}
fn app_with_profile(
    text: &str,
    project: Option<PathBuf>,
    sessions: Shared,
    profile: Option<PathBuf>,
) -> App {
    let now = Instant::now();
    let mut workspace = Workspace::new(Text::from(text), profile, project, false).unwrap();
    workspace
        .settings
        .set(Layer::User, "index_files", json!(false));
    let options = Options::default();
    let reporter = Reporter::for_options(&options, now).unwrap();
    App::new(
        options,
        Clock::new(now, std::time::SystemTime::now()),
        reporter,
        workspace,
        Arc::new(AtomicBool::new(false)),
        sessions,
    )
}
fn poll(app: &mut App) {
    app.poll_workspace_model();
    app.panels.poll(&mut app.workspace, 0..0);
    app.sync_active_view();
}
#[track_caller]
fn until(app: &mut App, predicate: impl Fn(&App) -> bool) {
    // A 25-megapixel image decode runs on a worker beside every other test in the suite;
    // on a loaded CI runner it has taken longer than 12 s (see the project's watchdog
    // guidance of 20-30 s for worker scheduling).
    until_within(app, Duration::from_secs(30), predicate)
}
/// A wait for work that is itself heavy: writing and decoding a 25-megapixel PNG takes
/// seconds on an unloaded machine and several times longer beside the rest of the suite,
/// so the decode's own start is given a wider deadline than ordinary worker scheduling.
#[track_caller]
fn until_within(app: &mut App, limit: Duration, predicate: impl Fn(&App) -> bool) {
    let deadline = Instant::now() + limit;
    loop {
        poll(app);
        if predicate(app) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "worker did not publish: {:?} ({})",
            app.workspace.tabs[app.workspace.active].path,
            app.panels.message
        );
        std::thread::sleep(Duration::from_millis(7));
    }
}
fn key(app: &mut App, chord: &str) {
    assert!(app.dispatch_key_binding(None, &Key::parse(chord).unwrap(), Instant::now(), 0..0));
}
fn rows(app: &App) -> Vec<PathBuf> {
    app.panels
        .side_rows
        .iter()
        .map(|r| r.path.clone())
        .collect()
}

fn prefs(root: &Path, values: &Value) -> PathBuf {
    let user = root.join("Packages/User");
    std::fs::create_dir_all(&user).unwrap();
    let path = user.join("Preferences.sublime-settings");
    std::fs::write(&path, values.to_string()).unwrap();
    path
}

#[test]
fn r20_sidebar_watch_refresh_and_patterns_sweep() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let profile = tempfile::tempdir().unwrap();
    let settings = prefs(profile.path(), &json!({"index_files":false}));
    std::fs::write(root.join(".gitignore"), "*.png\n").unwrap();
    let mut app = app_with_profile("", None, sessions(), Some(profile.path().into()));
    app.panels
        .add_folder(&mut app.workspace, root.clone())
        .unwrap();
    until(&mut app, |a| rows(a).contains(&root.join(".gitignore")));
    let mut expected = [false; 9];
    let mut rng = 1234u32;
    let mut excludes = false;
    for step in 0..36 {
        rng = rng.wrapping_mul(1664525).wrapping_add(1013904223);
        let i = (rng >> 16) as usize % expected.len();
        let path = root.join(format!("item{i}.png"));
        if expected[i] {
            std::fs::remove_file(&path).unwrap();
        } else {
            std::fs::write(&path, b"\0png").unwrap();
        }
        expected[i] = !expected[i];
        if step % 7 == 1 {
            excludes = !excludes;
            let files = if excludes { vec!["**/*.png"] } else { vec![] };
            std::fs::write(
                &settings,
                json!({"index_files":false,"file_exclude_patterns":files}).to_string(),
            )
            .unwrap();
            until(&mut app, |a| {
                a.workspace.settings.get("file_exclude_patterns") == Some(&json!(files))
            });
        }
        if step % 5 == 3 {
            app.run_named(None, "refresh_folder_list", &json!({}));
        }
        until(&mut app, |a| {
            let actual = rows(a);
            (0..expected.len()).all(|i| {
                actual.contains(&root.join(format!("item{i}.png"))) == (expected[i] && !excludes)
            })
        });
    }
}

#[test]
fn r20_recursive_sidebar_keys_and_window_visibility_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    std::fs::create_dir_all(root.join("one/two/three")).unwrap();
    std::fs::write(root.join("one/two/three/file"), "text").unwrap();
    std::os::unix::fs::symlink(&root, root.join("one/two/cycle")).unwrap();
    let other = tempfile::tempdir().unwrap();
    std::fs::write(other.path().join("private.txt"), "not in this project").unwrap();
    std::os::unix::fs::symlink(other.path(), root.join("one/external")).unwrap();
    let mut a = app("editing", None, sessions());
    a.panels.add_folder(&mut a.workspace, root.clone()).unwrap();
    a.panels.side_focused = true;
    a.panels.side_selected = 0;
    key(&mut a, "alt+right");
    until(&mut a, |a| {
        rows(a).contains(&root.join("one/two/three/file"))
    });
    assert!(!rows(&a).iter().any(|p| p.ends_with("cycle")));
    assert!(rows(&a).contains(&root.join("one/external")));
    assert!(!rows(&a).contains(&root.join("one/external/private.txt")));
    key(&mut a, "alt+left");
    assert_eq!(rows(&a), std::slice::from_ref(&root));
    let (mut window, _) = a.workspace.snapshot().unwrap();
    a.panels.capture_session(&mut window);
    let restored = serde_json::from_slice(&serde_json::to_vec(&window).unwrap()).unwrap();
    let mut b = app("", None, sessions());
    b.restore_ui(&restored);
    assert!(!b.panels.open_files_visible);
    a.run_named(None, "close_folder_list", &json!({}));
    assert!(a.workspace.folders.is_empty());
    assert!(rows(&a).is_empty());
}

#[test]
fn r20_recursive_sidebar_caps_expansion_and_cancels_on_collapse() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    for i in 0..5005 {
        std::fs::create_dir(root.join(format!("dir{i:04}"))).unwrap();
    }
    let mut a = app("", None, sessions());
    a.run_named(None, "add_folder", &json!({"dir":root}));
    until(&mut a, |a| a.panels.side_rows.len() == 5006);
    a.panels.side_focused = true;
    key(&mut a, "alt+right");
    key(&mut a, "alt+left");
    poll(&mut a);
    assert_eq!(rows(&a), [root]);
    key(&mut a, "alt+right");
    until(&mut a, |a| a.panels.message.contains("5,000 directories"));
    assert_eq!(a.panels.side_rows.len(), 5006);
}

#[test]
#[ignore = "2,000-directory headless expansion timing"]
fn r20_recursive_sidebar_timing() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    for i in 0..2000 {
        let folder = root.join(format!("d{i:04}"));
        std::fs::create_dir(&folder).unwrap();
        for j in 0..(3 + usize::from(i < 221)) {
            std::fs::write(folder.join(format!("file{j}.txt")), "fixture").unwrap();
        }
    }
    let mut a = app("", None, sessions());
    a.run_named(None, "add_folder", &json!({"dir":root}));
    until(&mut a, |a| a.panels.side_rows.len() == 2001);
    a.panels.side_focused = true;
    let start = Instant::now();
    key(&mut a, "alt+right");
    let mut worst = Duration::ZERO;
    loop {
        let t = Instant::now();
        poll(&mut a);
        worst = worst.max(t.elapsed());
        if a.panels.side_rows.len() == 8222 {
            break;
        }
        assert!(start.elapsed() < Duration::from_secs(30));
        std::thread::sleep(Duration::from_millis(2));
    }
    eprintln!(
        "recursive side bar: {} rows in {:.3}s, worst poll {:.2}ms",
        a.panels.side_rows.len(),
        start.elapsed().as_secs_f64(),
        worst.as_secs_f64() * 1000.
    );
}

#[test]
fn r20_add_folder_persists_relative_and_reveal_uses_link_spelling() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let project = root.join("test.sublime-project");
    let source = root.join("source");
    let external = tempfile::tempdir().unwrap();
    std::fs::create_dir(&source).unwrap();
    std::fs::write(&project, r#"{"folders":[{"path":"source"}],"future":true}"#).unwrap();
    std::fs::write(external.path().join("linked.rs"), "fn test() {}").unwrap();
    let alias = source.join("link");
    std::os::unix::fs::symlink(external.path(), &alias).unwrap();
    let mut a = app("", Some(project.clone()), sessions());
    let destination = root.join("added");
    std::fs::create_dir(&destination).unwrap();
    a.panels.review.native_context = true;
    a.run_named(None, "add_folder", &json!({"dir":destination}));
    until(&mut a, |app| {
        app.workspace.folders.iter().any(|f| f.path == destination)
    });
    let b = app("", Some(project.clone()), sessions());
    assert!(b.workspace.folders.iter().any(|f| f.path == destination));
    let model = vel_session::project::Project::load(&project).unwrap();
    assert_eq!(model.folders[1].path, Path::new("added"));
    assert_eq!(model.extra["future"], true);
    let lexical = alias.join("linked.rs");
    a.open_received_paths(std::slice::from_ref(&lexical))
        .unwrap();
    let original = a.workspace.tabs[a.workspace.active].path.clone().unwrap();
    let count = a.workspace.folders.len();
    a.panels
        .reveal_sidebar_path(&mut a.workspace, original)
        .unwrap();
    until(&mut a, |win| {
        rows(win).get(win.panels.side_selected) == Some(&lexical)
    });
    assert_eq!(a.workspace.folders.len(), count);
}

#[test]
fn r20_real_loader_opens_image_and_keeps_nonmatching_hex() {
    let dir = tempfile::tempdir().unwrap();
    let image = dir.path().join("two.png");
    image::RgbaImage::from_pixel(2, 3, image::Rgba([0, 120, 255, 255]))
        .save(&image)
        .unwrap();
    let bin = dir.path().join("data.bin");
    std::fs::copy(&image, &bin).unwrap();
    let mut a = app("", None, sessions());
    a.open_received_paths(&[image]).unwrap();
    until(&mut a, |a| a.view.image_preview.image().is_some());
    let preview = a.view.image_preview.image().unwrap();
    assert_eq!((preview.width, preview.height), (2, 3));
    assert!(a.workspace.editor().read_only);
    assert!(a.view.scene(a.workspace.editor()).layers.iter().flat_map(|l| &l.surfaces)
        .any(|s| matches!(s, vel_render::scene::Surface::Image(i) if i.image.logical_size == [2., 3.])));
    a.open_received_paths(&[bin]).unwrap();
    until(&mut a, |a| a.workspace.editor().binary.is_some());
    assert!(a.view.image_preview.image().is_none());
}

#[test]
fn r20_two_windows_recents_frame_and_close_rule() {
    let shared = sessions();
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let file = root.join("recent.txt");
    std::fs::write(&file, "from window A").unwrap();
    let mut a = app("", None, shared.clone());
    let mut b = app("test", None, shared.clone());
    a.open_received_paths(std::slice::from_ref(&file)).unwrap();
    until(&mut a, |a| {
        a.workspace.tabs[a.workspace.active].loading.is_none()
    });
    a.sync_recents();
    b.checkpoint(false).unwrap();
    let file_menu = b.application_menu();
    assert!(
        file_menu
            .iter()
            .find(|m| m.caption == "File")
            .unwrap()
            .children
            .iter()
            .flat_map(|m| &m.children)
            .flat_map(|m| &m.children)
            .any(|m| m.command.as_deref() == Some("open_recent_file")
                && m.caption == file.display().to_string())
    );
    assert_eq!(
        shared.borrow().test_snapshot().recent_files,
        std::slice::from_ref(&file)
    );
    let mut events = Vec::new();
    shared
        .borrow_mut()
        .update_system_recents(true, |url| events.push(url.map(Path::to_owned)));
    shared
        .borrow_mut()
        .update_system_recents(false, |url| events.push(url.map(Path::to_owned)));
    shared
        .borrow_mut()
        .update_system_recents(false, |url| events.push(url.map(Path::to_owned)));
    assert_eq!(events, [Some(file), None]);
    b.window_frame = Some([100., 300., 670., 440.]);
    b.panels.add_folder(&mut b.workspace, root).unwrap();
    b.workspace
        .settings
        .set(Layer::User, "close_windows_when_empty", json!(true));
    b.run_named(None, "close_all", &json!({}));
    assert!(b.workspace.is_empty() && !b.should_close_empty_window());
    b.run_named(None, "close_folder_list", &json!({}));
    assert!(b.should_close_empty_window());
    b.checkpoint(false).unwrap();
    let window = &shared.borrow().test_snapshot().windows[0];
    let restored: vel_session::Window =
        serde_json::from_slice(&serde_json::to_vec(window).unwrap()).unwrap();
    assert_eq!(restored.frame, b.window_frame);
    let invalid: vel_session::Window = serde_json::from_value(json!({"frame":"bad"})).unwrap();
    assert!(invalid.frame.is_none());
}

#[test]
fn r32_status_labels_binary_without_offering_text_syntax() {
    use crate::panels::STATUS;
    let files = tempfile::tempdir().unwrap();
    let bin = files.path().join("data.bin");
    std::fs::write(&bin, [0, 255, 0, 33]).unwrap();
    let mut a = app("", None, sessions());
    a.open_received_paths(&[bin]).unwrap();
    until(&mut a, |a| a.workspace.editor().binary.is_some());
    for (lang, label) in [("en", "Binary"), ("zh-Hans", "二进制")] {
        a.workspace
            .settings
            .set(Layer::User, "ui_language", json!(lang));
        a.workspace.sync_ui_language();
        a.content = a.panels.layout(a.window_size);
        let mut scene = vel_render::scene::Scene::default();
        a.panels.paint(&a.workspace, &mut scene);
        let top = a.window_size.height - STATUS * a.panels.ui_scale;
        let run = scene
            .layers
            .iter()
            .flat_map(|layer| &layer.text)
            .find(|run| f64::from(run.baseline) > top && run.text.as_str() == label)
            .expect(label);
        a.handle_mouse_event(
            None,
            MouseEvent {
                button: 0,
                kind: MouseKind::Down,
                position: Point {
                    x: f64::from(run.x) + 1.,
                    y: top + 5.,
                },
                click_count: 1,
                modifiers: Modifiers(0),
                timestamp: 0.,
            },
        );
        assert!(
            a.panels.overlay.is_none(),
            "{lang}: binary label is not a syntax control"
        );
    }
}

#[test]
fn r32_open_dialog_and_recent_menu_focus_without_repositioning() {
    let profile = tempfile::tempdir().unwrap();
    let files = tempfile::tempdir().unwrap();
    let first = files.path().canonicalize().unwrap().join("first:2.txt");
    let second = first.with_file_name("second.txt");
    std::fs::write(&first, "alpha\nbeta\n".repeat(200)).unwrap();
    std::fs::write(&second, "second\n").unwrap();
    let mut a = app_with_profile("", None, sessions(), Some(profile.path().into()));
    assert_eq!(a.workspace.tabs.len(), 1);
    assert!(a.workspace.tabs[0].path.is_none());
    assert!(a.workspace.editor().text.is_empty());
    assert!(!a.panels.side_visible);
    assert!(a.panels.overlay.is_none() && a.panels.panel.is_none());

    let invalid = files.path().join("socket");
    let _listener = std::os::unix::net::UnixListener::bind(&invalid).unwrap();
    a.open_file_choices = Some(vec![first.clone(), invalid, second.clone()]);
    key(&mut a, "super+o");
    assert!(
        a.session_due.is_some(),
        "valid paths still update the window"
    );
    assert!(a.panels.message.contains("only regular files and folders"));
    until(&mut a, |a| {
        a.workspace.tabs.len() == 2 && a.workspace.tabs.iter().all(|t| t.loading.is_none())
    });
    let first_index = a
        .workspace
        .tabs
        .iter()
        .position(|t| t.path.as_ref() == Some(&first))
        .unwrap();
    let second_index = a.workspace.active;
    let first_sheet = a.workspace.tabs[first_index].sheet;
    assert_eq!(a.workspace.tabs[second_index].path.as_ref(), Some(&second));
    a.workspace.select(first_index);
    a.sync_active_view();
    a.workspace.take_pending_center();
    a.workspace
        .editor_mut()
        .set_selection(vel_buffer::Selection::from_regions([
            vel_buffer::Region::new(6, 9),
        ]));
    let selection = a.workspace.editor().regions();
    a.view.scroll_y = a.view.line_height * 40.;
    let scroll = a.view.scroll_y;
    let open_recent = a
        .application_menu()
        .into_iter()
        .find(|row| row.id == "file")
        .unwrap()
        .children
        .into_iter()
        .find(|row| row.id == "open_recent")
        .unwrap()
        .children
        .into_iter()
        .flat_map(|row| row.children)
        .find(|row| {
            row.command.as_deref() == Some("open_recent_file")
                && row.caption == first.display().to_string()
        })
        .unwrap();
    let args = serde_json::from_str(&open_recent.args).unwrap();
    for from_menu in [false, true] {
        a.workspace.select(second_index);
        a.sync_active_view();
        if from_menu {
            a.run_named(None, open_recent.command.as_deref().unwrap(), &args);
        } else {
            a.open_file_choices = Some(vec![first.clone()]);
            key(&mut a, "super+o");
        }
        assert_eq!(a.workspace.tabs.len(), 2);
        assert_eq!(a.workspace.tabs[a.workspace.active].sheet, first_sheet);
        assert_eq!(a.workspace.editor().regions(), selection);
        assert_eq!(a.view.scroll_y, scroll);
        assert!(!a.workspace.take_pending_center());
    }
    let location = crate::cli::FileLocation::parse(&format!("{}:1:2", first.display()));
    assert!(
        crate::open_requests::open_locations(&mut a.workspace, &mut a.panels, [location])
            .is_empty()
    );
    assert_eq!(a.workspace.editor().caret, 1);
}

#[test]
fn r33_cli_local_forwarded_and_native_reopen_preserve_position() {
    use crate::instance::{Incoming, Instance, Request, Start};

    let profile = tempfile::tempdir().unwrap();
    let path = profile.path().join("source.txt:42");
    std::fs::write(&path, "alpha\nbeta\n".repeat(200)).unwrap();
    let cli = |suffix| {
        let args = [format!("{}{}", path.display(), suffix)];
        Options::parse(args).unwrap().unwrap()
    };
    let mut a = app("", None, sessions());
    let options = cli("");
    assert!(!options.files[0].has_position(), "literal colon filename");
    assert!(
        crate::open_requests::open_locations(&mut a.workspace, &mut a.panels, options.files)
            .is_empty()
    );
    a.sync_active_view();
    let file = a.workspace.active;
    let sheet = a.workspace.tabs[file].sheet;
    let bare = Request::new(&cli(""), None).unwrap();
    let Start::Primary(primary) = Instance::start_fixture(profile.path(), &bare).unwrap() else {
        panic!("expected fixture primary");
    };
    let forwarded = |request: &Request| {
        assert!(matches!(
            Instance::start_fixture(profile.path(), request).unwrap(),
            Start::Forwarded
        ));
        let mut requests = primary.drain();
        assert_eq!(requests.len(), 1);
        requests.pop().unwrap()
    };
    a.workspace.new_file().unwrap();
    let other = a.workspace.active;
    a.workspace.select(file);
    a.sync_active_view();
    a.workspace.editor_mut().set_caret(4, false);
    a.view.scroll_y = a.view.line_height * 40.;
    let scroll = a.view.scroll_y;
    for incoming in [
        Incoming::local(bare.clone()),
        forwarded(&bare),
        Incoming::local(crate::open_requests::native_request(std::slice::from_ref(&path)).unwrap()),
    ] {
        a.workspace.select(other);
        a.sync_active_view();
        a.accept_request(incoming).unwrap();
        a.changed_state(Instant::now(), false);
        assert_eq!(a.workspace.tabs.len(), 2);
        assert_eq!(a.workspace.tabs[a.workspace.active].sheet, sheet);
        assert_eq!(a.workspace.editor().caret, 4);
        assert_eq!(a.view.scroll_y, scroll);
        assert!(!a.workspace.take_pending_center());
    }
    for (suffix, caret) in [(":1:1", 0), (":2", 6), (":2:2", 7)] {
        let request = Request::new(&cli(suffix), None).unwrap();
        a.accept_request(forwarded(&request)).unwrap();
        assert_eq!(a.workspace.editor().caret, caret);
        assert!(a.workspace.take_pending_center());
    }
}

#[test]
fn r33_status_fields_and_switcher_follow_settings() {
    use crate::panels::StatusItem;

    let mut a = app("status controls", None, sessions());
    a.scene();
    for (setting, item, other) in [
        ("show_syntax", StatusItem::Syntax, StatusItem::Indentation),
        (
            "show_indentation",
            StatusItem::Indentation,
            StatusItem::Syntax,
        ),
    ] {
        assert!(
            a.panels.status_item(item).is_some(),
            "{setting} defaults on"
        );
        a.workspace
            .editor_mut()
            .settings
            .set(Layer::View, setting, json!(false));
        a.scene();
        assert!(a.panels.status_item(item).is_none());
        assert!(a.panels.status_item(other).is_some());
        a.workspace
            .editor_mut()
            .settings
            .set(Layer::View, setting, json!(true));
        a.scene();
    }
    let hit = a.panels.status_switcher_rect(&a.workspace).unwrap();
    let click = |button| MouseEvent {
        button,
        kind: MouseKind::Down,
        position: Point {
            x: hit.x + hit.width / 2.,
            y: hit.y + hit.height / 2.,
        },
        click_count: 1,
        modifiers: Modifiers::default(),
        timestamp: 0.,
    };
    let sidebar = a.panels.side_visible;
    a.handle_mouse_event(None, click(0));
    assert_ne!(a.panels.side_visible, sidebar);
    a.handle_mouse_event(None, click(0));
    assert_eq!(a.panels.side_visible, sidebar);

    a.workspace
        .settings
        .set(Layer::User, "ui_language", json!("zh-Hans"));
    a.workspace.sync_ui_language();
    a.scene();
    a.handle_mouse_event(None, click(1));
    assert_eq!(a.panels.side_visible, sidebar);
    assert_eq!(a.last_status_switcher_menu[0].caption, "控制台");
    let build = a.last_status_switcher_menu[1].clone();
    a.run_named(
        None,
        build.command.as_deref().unwrap(),
        &serde_json::from_str(&build.args).unwrap(),
    );
    assert_eq!(a.panels.panel, Some(crate::panels::Panel::Output));

    a.workspace
        .editor_mut()
        .settings
        .set(Layer::View, "show_sidebar_button", json!(false));
    a.scene();
    assert!(a.panels.status_switcher_rect(&a.workspace).is_none());
    a.last_status_switcher_menu.clear();
    a.handle_mouse_event(None, click(0));
    a.handle_mouse_event(None, click(1));
    assert_eq!(a.panels.side_visible, sidebar);
    assert!(a.last_status_switcher_menu.is_empty());
}

#[test]
fn r33_background_reload_clean_active_inactive_and_dirty() {
    let files = tempfile::tempdir().unwrap();
    let path = files.path().canonicalize().unwrap().join("watched.txt");
    for (background, active, dirty) in [
        (true, false, false),
        (false, true, false),
        (false, false, false),
        (false, false, true),
        (false, true, true),
    ] {
        std::fs::write(&path, "original").unwrap();
        let mut a = app("", None, sessions());
        let w = &mut a.workspace;
        w.settings
            .set(Layer::User, "reload_file_in_background", json!(background));
        w.open(crate::cli::FileLocation::path_only(path.clone()))
            .unwrap();
        let document = w.active;
        if dirty {
            w.editor_mut()
                .run("insert", &json!({"characters":"draft "}))
                .unwrap();
        }
        if !active {
            w.new_file().unwrap();
        }
        std::fs::write(&path, "new file contents").unwrap();
        w.apply_file_changes(vec![Ok(vec![path.clone()])]);
        let tab = &w.tabs[document];
        if dirty {
            assert_eq!(tab.editor.text, "draft original");
            assert_eq!(tab.external.as_ref().unwrap().text, "new file contents");
        } else if background || active {
            assert_eq!(tab.editor.text, "new file contents");
        } else {
            assert_eq!(tab.editor.text, "original");
            assert!(tab.external.is_none());
            w.poll();
            assert_eq!(w.tabs[document].editor.text, "original");
            // Multiple writes while inactive coalesce, even if the last notification
            // has not arrived by the time the user returns to the file.
            std::fs::write(&path, "latest contents on disk").unwrap();
            w.select(document);
            w.poll();
            assert_eq!(w.editor().text, "latest contents on disk");
        }
    }
}

#[test]
fn r33_background_reload_follows_shared_buffer_across_clones() {
    let files = tempfile::tempdir().unwrap();
    let path = files.path().canonicalize().unwrap().join("shared.txt");
    std::fs::write(&path, "original").unwrap();
    let mut a = app("", None, sessions());
    let w = &mut a.workspace;
    w.settings
        .set(Layer::User, "reload_file_in_background", json!(false));
    w.open(crate::cli::FileLocation::path_only(path.clone()))
        .unwrap();
    let original = w.active;
    w.clone_file().unwrap();
    let clone = w.active;
    // Rendering another visible clone can temporarily put ownership there.
    w.prepare_sheet(original).unwrap();
    assert!(w.tabs[clone].editor.shared_shadow);
    std::fs::write(&path, "first disk change").unwrap();
    w.apply_file_changes(vec![Ok(vec![path.clone()])]);
    w.reconcile_installs();
    for i in [original, clone] {
        assert_eq!(w.tabs[i].editor.text, "first disk change");
    }

    w.select(original);
    w.new_file().unwrap();
    std::fs::write(&path, "second, longer disk change").unwrap();
    w.apply_file_changes(vec![Ok(vec![path])]);
    for i in [original, clone] {
        assert_eq!(w.tabs[i].editor.text, "first disk change");
    }
    w.select(clone);
    w.poll();
    assert_eq!(w.tabs[original].document, w.tabs[clone].document);
    for i in [original, clone] {
        assert_eq!(w.tabs[i].editor.text, "second, longer disk change");
    }
}

#[test]
fn r32_cmd_w_keeps_a_named_project_without_folders() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("empty.sublime-project");
    std::fs::write(&project, r#"{"folders":[]}"#).unwrap();
    for (project, should_close) in [(Some(project), false), (None, true)] {
        let mut a = app("", project.clone(), sessions());
        a.workspace
            .settings
            .set(Layer::User, "close_windows_when_empty", json!(true));
        assert!(a.workspace.folders.is_empty());
        assert_eq!(a.workspace.project_path().is_some(), project.is_some());
        assert_eq!(a.workspace.tabs.len(), 1);
        key(&mut a, "super+w");
        assert!(a.workspace.is_empty());
        assert_eq!(a.empty_close_requested, should_close, "{project:?}");
    }
}

#[test]
fn r20_menu_roles_and_line_ending_state() {
    let mut a = app("text", None, sessions());
    let menu = a.application_menu();
    let window = menu.iter().find(|item| item.caption == "Window").unwrap();
    for role in [
        "native_minimize",
        "native_zoom",
        "native_bring_all_to_front",
    ] {
        assert!(
            window
                .children
                .iter()
                .any(|item| item.command.as_deref() == Some(role))
        );
    }
    let endings = menu
        .iter()
        .find(|item| item.caption == "View")
        .unwrap()
        .children
        .iter()
        .find(|item| item.caption == "Line Endings")
        .unwrap();
    for (format, argument) in [
        (vel_fileio::LineEnding::Lf, "unix"),
        (vel_fileio::LineEnding::CrLf, "windows"),
        (vel_fileio::LineEnding::Cr, "cr"),
    ] {
        a.workspace.tabs[0].format.line_ending = format;
        assert_eq!(
            endings
                .children
                .iter()
                .filter(|item| a.command_checked("set_line_ending", &item.args))
                .map(
                    |item| serde_json::from_str::<Value>(&item.args).unwrap()["type"]
                        .as_str()
                        .unwrap()
                        .to_owned()
                )
                .collect::<Vec<_>>(),
            [argument]
        );
    }
}

#[test]
fn r20_tab_transfer_keeps_dirty_selection_and_undo() {
    let shared = sessions();
    let fixtures = tempfile::tempdir().unwrap();
    let make = |tab_size, word_wrap, name, keys: &str, shared| {
        let profile = fixtures.path().join(name);
        let settings = prefs(
            &profile,
            &json!({"index_files":false,"detect_indentation":false,"fixture_profile":name}),
        );
        let user = settings.parent().unwrap();
        std::fs::write(
            user.join("Plain Text.sublime-settings"),
            json!({"fixture_syntax":name}).to_string(),
        )
        .unwrap();
        // Only the documented User keymap names are loaded (key_bindings docs).
        std::fs::write(user.join("Default (OSX).sublime-keymap"), keys).unwrap();
        let project = profile.join("transfer.sublime-project");
        std::fs::write(
            &project,
            json!({"settings":{"tab_size":tab_size,"word_wrap":word_wrap}}).to_string(),
        )
        .unwrap();
        app_with_profile("", Some(project), shared, Some(profile))
    };
    let mut a = make(7, true, "A", "[]", shared.clone());
    let mut b = make(
        3,
        false,
        "B",
        r#"[{"keys":["ctrl+9"],"command":"insert","args":{"characters":"destination"}}]"#,
        shared,
    );
    a.run_named(None, "insert", &json!({"characters":"unsaved"}));
    a.workspace
        .editor_mut()
        .settings
        .set(Layer::View, "fixture_view", json!(42));
    let sheet = a.workspace.tabs[a.workspace.active].sheet;
    let mut rng = 3u32;
    for turn in 0..31 {
        rng = rng.wrapping_mul(1664525).wrapping_add(1013904223);
        let (source, dest) = if a.workspace.sheet_index(sheet).is_some() {
            (&mut a, &mut b)
        } else {
            (&mut b, &mut a)
        };
        let at = ((rng >> 24) % 8) as usize;
        source.workspace.editor_mut().set_caret(at, false);
        source.run_named(
            None,
            "move_tab_to_window",
            &json!({"sheet": sheet, "window": dest.session_id}),
        );
        poll(dest);
        assert!(source.workspace.sheet_index(sheet).is_none());
        assert_eq!(dest.workspace.editor().text.to_string(), "unsaved");
        assert_eq!(dest.workspace.editor().caret, at);
        assert!(dest.workspace.tabs[dest.workspace.active].dirty());
        let (size, wrap, profile) = if turn % 2 == 0 {
            (3, false, "B")
        } else {
            (7, true, "A")
        };
        let settings = &dest.workspace.editor().settings;
        assert_eq!(settings.get("tab_size"), Some(&json!(size)));
        assert_eq!(settings.get("word_wrap"), Some(&json!(wrap)));
        assert_eq!(settings.get("fixture_profile"), Some(&json!(profile)));
        assert_eq!(settings.get("fixture_syntax"), Some(&json!(profile)));
        assert_eq!(settings.get("fixture_view"), Some(&json!(42)));
    }
    b.run_named(None, "undo", &json!({}));
    assert!(b.workspace.editor().text.is_empty());
    key(&mut b, "ctrl+9");
    assert_eq!(b.workspace.editor().text.to_string(), "destination");
}

#[test]
fn r20_invisible_cells_and_caret() {
    let mut a = app("a\u{200b}b\u{202e}c\u{1}", None, sessions());
    a.view.apply_style(a.workspace.editor());
    let before = a.view.rect_at(a.workspace.editor(), 1);
    let normal = a.view.rect_at(a.workspace.editor(), 4).x;
    for (key, value) in [
        ("draw_unicode_white_space", json!("all")),
        ("draw_unicode_bidi", json!(true)),
        ("draw_control_characters", json!(true)),
    ] {
        a.workspace
            .editor_mut()
            .settings
            .set(Layer::View, key, value);
    }
    let scene = a.view.scene(a.workspace.editor());
    let after = a.view.rect_at(a.workspace.editor(), 4);
    assert!(after.x > normal + a.view.cell * 4.);
    let hit = |fraction| Point {
        x: before.x + (after.x - before.x) * fraction,
        y: after.y + 2.,
    };
    assert_eq!(a.view.hit_test(a.workspace.editor(), hit(0.49)), 1);
    assert_eq!(a.view.hit_test(a.workspace.editor(), hit(0.51)), 4);
    let runs: Vec<_> = scene
        .layers
        .iter()
        .flat_map(|layer| &layer.text)
        .map(|run| run.text.as_str())
        .collect();
    for label in ["<0x200b>", "<0x202e>", "<0x01>"] {
        assert!(runs.contains(&label), "{runs:?}");
    }
    for expected in [1, 4, 5, 8, 9, 10] {
        key(&mut a, "right");
        assert_eq!(a.workspace.editor().caret, expected);
    }
}

#[test]
fn r20_multicaret_invisible_wrap_preserves_segments_and_gaps() {
    let line = "aa\u{200b} b\tcc ddd\u{a0}eee fff";
    let mut a = app(&format!("{line}\n").repeat(12), None, sessions());
    for (key, value) in [
        ("word_wrap", json!(true)),
        ("wrap_width", json!(13)),
        ("draw_unicode_white_space", json!("all")),
    ] {
        a.workspace
            .editor_mut()
            .settings
            .set(Layer::View, key, value);
    }
    let editor = a.workspace.editor_mut();
    let selection = vel_buffer::Selection::from_regions(
        (0..12).map(|row| vel_buffer::Region::caret(editor.text.line_to_byte(row) + line.len())),
    );
    editor.set_selection(selection);
    a.view.apply_style(a.workspace.editor());
    for characters in ["x", "y", "\u{200b}", "z ", "\u{202f}", "rst", "\u{301}"]
        .into_iter()
        .cycle()
        .take(49)
    {
        a.run_named(None, "insert", &json!({"characters":characters}));
        a.view.apply_style(a.workspace.editor());
        let editor = a.workspace.editor();
        let options = crate::view::motion_layout_options(editor);
        let mut reference = crate::layout::Layout::default();
        reference.set_invisibles(vel_render::invisibles::Invisible {
            whitespace: vel_render::invisibles::WhiteSpace::All,
            ..Default::default()
        });
        reference.configure(
            &editor.text,
            (editor.buffer.id(), editor.buffer.change_id()),
            options,
            &[],
        );
        let layout = editor.layout.borrow();
        assert!(layout.wrapped());
        assert_eq!(layout.total(), reference.total());
        for visual in 0..layout.total() {
            let (row, segment) = layout.locate(visual);
            assert_eq!((row, segment), reference.locate(visual));
            assert_eq!(
                layout.segment(&editor.text, row, segment),
                reference.segment(&editor.text, row, segment)
            );
            let text = editor.text.line(row).to_string();
            assert_eq!(
                layout.invisible_gaps(row, &text, 0, 0),
                reference.invisible_gaps(row, &text, 0, 0)
            );
            if row < 12 {
                assert!(layout.invisible_gaps(row, &text, 0, 0).is_some());
            }
        }
    }
}

#[test]
fn r20_hidden_image_decode_is_cancelled_and_folder_edits_are_independent() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let image = root.join("IMAGE.PNG");
    image::RgbImage::new(5000, 5000)
        .save_with_format(&image, image::ImageFormat::Png)
        .unwrap();
    let folder = root.join("folder");
    std::fs::create_dir(&folder).unwrap();
    let project = root.join("image.sublime-project");
    std::fs::write(&project, "{}").unwrap();
    let mut a = app("", Some(project.clone()), sessions());
    a.panels.review.native_context = true;
    a.run_named(None, "open_file", &json!({"file":image}));
    until_within(&mut a, Duration::from_secs(180), |a| {
        a.view.image_preview.busy()
    });
    let cancelled = a.view.image_preview.cancellation_flag().unwrap();
    assert!(!cancelled.load(Ordering::Acquire));
    a.run_named(None, "add_folder", &json!({"dir":folder}));
    a.run_named(None, "new_file", &json!({}));
    assert!(cancelled.load(Ordering::Acquire));
    until(&mut a, |a| {
        a.workspace.folders.iter().any(|f| f.path == folder)
    });
    assert!(a.view.image_preview.image().is_none());
    assert_eq!(
        vel_session::project::Project::load(&project)
            .unwrap()
            .folders[0]
            .path,
        Path::new("folder")
    );
}

#[test]
fn r20_hex_and_image_carets_do_not_break_navigation() {
    let dir = tempfile::tempdir().unwrap();
    let mut a = app("starting text", None, sessions());
    let image = dir.path().join("PHOTO.JPG");
    image::RgbImage::new(8, 9)
        .save_with_format(&image, image::ImageFormat::Jpeg)
        .unwrap();
    let hex = dir.path().join("data.bin");
    std::fs::write(&hex, [0, 0xff, 0xfe, 0, 0xf7, 10, 13].repeat(19)).unwrap();
    for (i, path) in [image, hex].into_iter().enumerate() {
        a.run_named(None, "open_file", &json!({"file":path}));
        until(&mut a, |a| a.workspace.editor().binary.is_some());
        if i == 0 {
            until(&mut a, |a| a.view.image_preview.image().is_some());
        }
        key(&mut a, "right");
        let byte = a.workspace.editor().caret;
        assert!(byte > 0);
        for chord in [
            "super+e",
            "super+shift+e",
            "super+alt+g",
            "super+alt+shift+g",
        ] {
            key(&mut a, chord);
            assert_eq!(a.workspace.editor().caret, byte);
            assert!(a.panels.message.contains("Text search is unavailable"));
        }
        key(&mut a, "super+a");
        assert!(a.workspace.editor().caret > 0);
        a.run_named(None, "show_panel", &json!({"panel":"find"}));
        a.run_named(None, "hide_panel", &json!({"cancel":true}));
        a.run_named(None, "move_to", &json!({"to":"eof"}));
        a.run_named(None, "goto_line", &json!({}));
        std::hint::black_box(a.scene());
        assert!(a.panels.input().is_some());
        key(&mut a, "super+a");
        type_in_panel(&mut a, "0x40");
        key(&mut a, "enter");
        assert_eq!(a.workspace.editor().caret, 0x40);
        for query in [":5", "@", "#word"] {
            key(&mut a, "super+p");
            type_in_panel(&mut a, query);
            std::hint::black_box(a.scene());
            assert!(a.panels.message.contains("Text navigation is unavailable"));
            assert_eq!(a.workspace.editor().caret, 0x40);
            key(&mut a, "escape");
        }
        key(&mut a, "left");
        a.run_named(None, "show_overlay", &json!({"overlay":"goto"}));
        key(&mut a, "escape");
        let document = dir.path().join(format!("new{i}.txt"));
        std::fs::write(&document, "ordinary text").unwrap();
        a.run_named(None, "open_file", &json!({"file":document}));
        until(&mut a, |a| a.workspace.editor().text == "ordinary text");
        a.run_named(None, "jump_back", &json!({}));
        a.run_named(None, "next_view", &json!({}));
        a.run_named(None, "prev_view", &json!({}));
    }
}

fn type_in_panel(a: &mut App, text: &str) {
    assert!(a.panels.text_input(
        &mut a.workspace,
        vel_platform::TextInput::Insert {
            text: text.into(),
            replacement: None
        },
        0..0,
    ));
    a.changed_state(Instant::now(), true);
}

#[test]
#[ignore = "exercises the whole packaged command palette and keymap on binary sheets"]
fn round20_hex_image_default_command_sweep() {
    let dir = tempfile::tempdir().unwrap();
    let profile = tempfile::tempdir().unwrap();
    let settings = prefs(
        profile.path(),
        &json!({"index_files":false,"update_system_recent_files":false}),
    );
    let image = dir.path().join("picture.png");
    image::RgbImage::new(10, 12).save(&image).unwrap();
    // Recognized binary signatures keep the repeating payload from looking
    // like BOM-less UTF-16 to the real loader.
    let payload = [0, 0xff, 0xfe, 0, 0xf7, 10, 13].repeat(270);
    let binary = [b"\x7fELF".as_slice(), &payload].concat();
    let corrupt = [b"\x89PNG\r\n\x1a\n".as_slice(), &payload].concat();
    std::fs::write(dir.path().join("data.bin"), &binary).unwrap();
    std::fs::write(dir.path().join("corrupt.png"), &corrupt).unwrap();
    let map = vel_config::json::Document::parse(include_str!(
        "../resources/Packages/Default/Velocity.sublime-keymap"
    ))
    .unwrap();
    let bindings = map.value.as_array().unwrap();
    assert!(!bindings.is_empty());
    let root = dir.path().canonicalize().unwrap();
    for name in ["data.bin", "picture.png", "corrupt.png"] {
        let file = root.join(name);
        for position in ["right", "super+a", "eof"] {
            let mut a = app_with_profile("", None, sessions(), Some(profile.path().into()));
            let palette = a.workspace.palette.clone();
            assert!(palette.len() > 100);
            for (i, binding) in bindings
                .iter()
                .map(Some)
                .chain(palette.iter().map(|_| None))
                .enumerate()
            {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    a.panels.cancel_overlay(&mut a.workspace);
                    a.run_named(None, "hide_panel", &json!({"cancel":true}));
                    // Reopen with Encoding and similar commands may turn the fixture
                    // into a text tab. Do not carry that choice into the next case.
                    if let Some(index) = a.workspace.tabs.iter().position(|tab| {
                        tab.path.as_deref() == Some(&file)
                            && (tab.editor.binary.is_none() || tab.loading.is_some())
                    }) {
                        a.workspace.close(index).unwrap();
                    }
                    a.run_named(None, "open_file", &json!({"file":file}));
                    until(&mut a, |a| {
                        let tab = &a.workspace.tabs[a.workspace.active];
                        tab.path.as_deref() == Some(&file) && tab.editor.binary.is_some()
                    });
                    if name == "picture.png" {
                        until(&mut a, |a| a.view.image_preview.image().is_some());
                    }
                    a.panels.focus_editor_for_context();
                    a.workspace.editor_mut().keymap.cancel();
                    a.run_named(None, "move_to", &json!({"to":"bof"}));
                    if position == "eof" {
                        a.run_named(None, "move_to", &json!({"to":"eof"}));
                    } else {
                        key(&mut a, position);
                    }
                    assert!(a.workspace.editor().caret > 0);
                    if let Some(binding) = binding {
                        for part in binding["keys"].as_array().unwrap() {
                            a.dispatch_key_binding(
                                None,
                                &Key::parse(part.as_str().unwrap()).unwrap(),
                                Instant::now(),
                                0..0,
                            );
                        }
                    } else {
                        let command = &palette[i - bindings.len()].invocation;
                        a.run_named(None, &command.command, &command.args);
                    }
                    std::hint::black_box(a.scene());
                }));
                if let Err(payload) = result {
                    eprintln!(
                        "panicked for {name}, {position}, {:?}, {:?}",
                        binding,
                        i.checked_sub(bindings.len()).and_then(|i| palette.get(i)),
                    );
                    std::panic::resume_unwind(payload);
                }
            }
            // Restore options a palette command may have changed before the next app.
            std::fs::write(
                &settings,
                json!({"index_files":false,"update_system_recent_files":false}).to_string(),
            )
            .unwrap();
            eprintln!(
                "{name}, {position}: {} bindings and {} palette entries",
                bindings.len(),
                palette.len()
            );
        }
    }
}

#[test]
fn r20_path_patterns_preserve_required_slashes_and_project_root() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let fixtures = [
        "a/b/direct.rs",
        "x/gen/xg.rs",
        "src/gen/sg.rs",
        "root.rs",
        "a/middle/b/hidden.rs",
        "x/middle/gen/hidden.rs",
        "src/middle/gen/hidden.rs",
    ];
    for relative in fixtures {
        let path = root.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "needle").unwrap();
    }
    let profile = tempfile::tempdir().unwrap();
    prefs(
        profile.path(),
        &json!({
            "index_files":false,
            "folder_exclude_patterns":["a/*/b", "x/**/gen", "src/**/gen"]
        }),
    );
    let mut a = app_with_profile("", None, sessions(), Some(profile.path().into()));
    a.run_named(None, "add_folder", &json!({"dir":root}));
    until(&mut a, |a| a.workspace.folders.len() == 1);
    a.run_named(None, "find_in_files", &json!({"text":"needle"}));
    key(&mut a, "enter");
    until(&mut a, |a| a.panels.file_complete);
    let mut found: Vec<_> = a
        .panels
        .result_view
        .rows
        .iter()
        .filter_map(|row| match row {
            vel_search::files::Row::Header(path) => Some(path.strip_prefix(&root).unwrap()),
            _ => None,
        })
        .collect();
    found.sort_unstable();
    assert_eq!(
        found,
        ["a/b/direct.rs", "root.rs", "src/gen/sg.rs", "x/gen/xg.rs"].map(Path::new)
    );

    // A separate project with file exclusions must still find first-level files.
    for pattern in ["//*/", "//**/"] {
        let project = root.join("settings.sublime-project");
        std::fs::write(
            &project,
            json!({"folders":[{
                "path":".","file_exclude_patterns":[pattern],
            }]})
            .to_string(),
        )
        .unwrap();
        let mut a = app_with_profile("", Some(project), sessions(), Some(profile.path().into()));
        key(&mut a, "super+p");
        type_in_panel(&mut a, "root.rs");
        until(&mut a, |a| {
            !a.panels.quick_pending && a.panels.quick.current().is_some()
        });
        key(&mut a, "enter");
        until(&mut a, |a| a.workspace.editor().text == "needle");
        assert_eq!(
            a.workspace.tabs[a.workspace.active].path.as_ref(),
            Some(&root.join("root.rs"))
        );
    }
}

#[test]
fn r20_find_in_files_where_uses_shared_patterns() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    for relative in [
        "root.rs",
        "one/a/file.rs",
        "nested/a/file.rs",
        "nested/a/d/e/file.rs",
        "x/gen/gen.rs",
        "x/sub/gen/no.rs",
        "nested/[ab].rs",
        "nested/a.rs",
        "nested/{a,b}.rs",
    ] {
        let path = root.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "needle").unwrap();
    }
    let mut a = app("", None, sessions());
    a.run_named(None, "add_folder", &json!({"dir":root}));
    until(&mut a, |a| a.workspace.folders.len() == 1);
    a.run_named(None, "find_in_files", &json!({"text":"needle"}));
    key(&mut a, "tab"); // Replace
    key(&mut a, "tab"); // Where
    type_in_panel(
        &mut a,
        "<project>, a/**/file.rs, gen.rs, [ab].rs, \"{a,b}.rs\", -x/**/gen",
    );
    assert!(a.panels.where_input.value().contains("a/**/file.rs"));
    key(&mut a, "enter");
    until(&mut a, |a| a.panels.file_complete);
    let mut found: Vec<_> = a
        .panels
        .result_view
        .rows
        .iter()
        .filter_map(|row| match row {
            vel_search::files::Row::Header(path) => Some(path.strip_prefix(&root).unwrap()),
            _ => None,
        })
        .collect();
    found.sort_unstable();
    assert_eq!(
        found,
        [
            "nested/[ab].rs",
            "nested/a/d/e/file.rs",
            "nested/{a,b}.rs",
            "x/gen/gen.rs"
        ]
        .map(Path::new)
    );
}

#[test]
fn r20_reveal_outside_project_is_transient_until_explicit_add() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let project = root.join("p.sublime-project");
    let source = "{\n\t\"folders\": [\n\t\t{ \"path\": \"inside\" }, // existing root\n\t],\n\t/* keep this */ \"setting\": 42,\n}\n";
    std::fs::create_dir(root.join("inside")).unwrap();
    std::fs::create_dir(root.join("outside")).unwrap();
    std::fs::write(&project, source).unwrap();
    let file = root.join("outside/document.txt");
    std::fs::write(&file, "outside").unwrap();
    let mut a = app("", Some(project.clone()), sessions());
    a.panels.review.native_context = true;
    a.run_named(None, "open_file", &json!({"file":file}));
    until(&mut a, |a| a.workspace.editor().text == "outside");
    a.run_named(None, "reveal_in_side_bar", &json!({"path":file}));
    until(&mut a, |a| {
        rows(a).get(a.panels.side_selected) == Some(&file)
    });
    assert_eq!(std::fs::read_to_string(&project).unwrap(), source);
    assert_eq!(
        a.workspace.snapshot().unwrap().0.folders,
        [root.join("inside")]
    );
    let alias = root.join("alias");
    std::os::unix::fs::symlink(root.join("outside"), &alias).unwrap();
    a.run_named(None, "add_folder", &json!({"dir":alias}));
    until(&mut a, |a| {
        a.workspace.folders.iter().any(|f| f.path == alias)
    });
    let saved = std::fs::read_to_string(&project).unwrap();
    assert!(saved.contains("\t\t{ \"path\": \"inside\" }, // existing root"));
    assert!(saved.contains("\t/* keep this */ \"setting\": 42,"));
    let fresh = app("", Some(project), sessions());
    assert_eq!(
        fresh
            .workspace
            .folders
            .iter()
            .map(|f| f.path.clone())
            .collect::<Vec<_>>(),
        [root.join("inside"), alias]
    );
}

#[test]
fn r20_selection_and_shift_anchor_survive_external_inserts() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    for file in ["n.txt", "z.txt"] {
        std::fs::write(root.join(file), file).unwrap();
    }
    let profile = tempfile::tempdir().unwrap();
    prefs(
        profile.path(),
        &json!({"index_files":false,"animation_enabled":false,"preview_on_click":false}),
    );
    let mut a = app_with_profile("", None, sessions(), Some(profile.path().into()));
    a.run_named(None, "add_folder", &json!({"dir":root}));
    until(&mut a, |a| rows(a).contains(&root.join("n.txt")));
    fn click(a: &mut App, path: &Path, extend: bool) {
        a.scene();
        let pt = a.panels.test_sidebar_point(&a.workspace, path);
        assert!(
            a.panels
                .mouse(&mut a.workspace, pt, true, false, extend, 0..0)
                .unwrap()
        );
    }
    click(&mut a, &root.join("n.txt"), false);
    for file in ["a.txt", "b.txt"] {
        std::fs::write(root.join(file), file).unwrap();
    }
    until(&mut a, |a| rows(a).contains(&root.join("b.txt")));
    key(&mut a, "enter");
    assert_eq!(
        a.workspace.tabs[a.workspace.active].path.as_ref(),
        Some(&root.join("n.txt"))
    );
    click(&mut a, &root.join("z.txt"), true);
    assert!(
        a.workspace
            .tabs
            .iter()
            .any(|tab| tab.path.as_ref() == Some(&root.join("z.txt")))
    );
    assert!(
        !a.workspace
            .tabs
            .iter()
            .any(|tab| tab.path.as_ref() == Some(&root.join("b.txt")))
    );
}

#[test]
fn r20_close_selected_save_and_last_transfer_request_empty_window_close() {
    let dir = tempfile::tempdir().unwrap();
    let project_file = dir.path().join("saved.txt");
    for name in [
        "save_and_close",
        "close_selected_tabs",
        "move_tab_to_window",
    ] {
        let shared = sessions();
        let mut a = app("edited", None, shared.clone());
        let mut b = app("", None, shared);
        a.workspace
            .editor_mut()
            .settings
            .set(Layer::View, "close_windows_when_empty", json!(true));
        a.workspace
            .settings
            .set(Layer::User, "close_windows_when_empty", json!(true));
        a.run_named(
            None,
            name,
            &json!({"file":project_file,"window":b.session_id,"force":true}),
        );
        assert!(
            a.workspace.is_empty() && a.empty_close_requested,
            "{name}: {}",
            a.panels.message
        );
        poll(&mut b);
        if name == "move_tab_to_window" {
            assert_eq!(b.workspace.editor().text.to_string(), "edited");
        }
    }
    let frame: vel_session::Window =
        serde_json::from_value(json!({"frame":[1e308,-5,0.5,1e-9]})).unwrap();
    assert_eq!(frame.frame, Some([1_000_000., -5., 320., 200.]));
    let windows: Vec<vel_session::Window> =
        serde_json::from_str(r#"[{"frame":[1e999,0,500,600]},{"frame":[1,2,500,600]}]"#).unwrap();
    assert!(windows[0].frame.is_none());
    assert_eq!(windows[1].frame, Some([1., 2., 500., 600.]));
}
