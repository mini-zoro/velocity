use super::*;
use crate::cli::FileLocation;
use std::{cell::RefCell, fs, rc::Rc};
use vel_config::{keymap::Key, settings::Layer};

fn app(language: &str) -> App {
    let options = Options::default();
    let now = Instant::now();
    let mut workspace = Workspace::new_empty(None, None, false).unwrap();
    workspace
        .settings
        .set(Layer::User, "ui_language", json!(language));
    workspace.sync_ui_language();
    workspace
        .settings
        .set(Layer::User, "tree_animation_enabled", json!(false));
    App::new(
        options.clone(),
        Clock::new(now, std::time::SystemTime::now()),
        Reporter::for_options(&options, now).unwrap(),
        workspace,
        Arc::new(AtomicBool::new(false)),
        Rc::new(RefCell::new(crate::session::Sessions::new(None))),
    )
}

fn key(app: &mut App, chord: &str) {
    assert!(app.dispatch_key_binding(None, &Key::parse(chord).unwrap(), Instant::now(), 0..0));
}

fn until(app: &mut App, ready: impl Fn(&App) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        app.workspace.poll();
        app.panels.poll(&mut app.workspace, 0..0);
        if ready(app) {
            return;
        }
        assert!(Instant::now() < deadline, "fixture did not settle");
        std::thread::park_timeout(Duration::from_millis(1));
    }
}

fn goto(app: &mut App, query: &str, path: &Path) {
    app.run_named(
        None,
        "show_overlay",
        &json!({"overlay":"goto", "text":query}),
    );
    until(app, |a| {
        !a.panels.ranking_pending()
            && a.workspace.tabs[a.workspace.active].path.as_deref() == Some(path)
    });
}

#[test]
fn r43_empty_titles_survive_goto_in_both_languages() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap().join("folder");
    fs::create_dir(&root).unwrap();
    let path = root.join("zh.md");
    fs::write(&path, "# preview\n").unwrap();
    for (language, untitled) in [("en", "untitled"), ("zh-Hans", "未命名")] {
        let mut a = app(language);
        assert!(a.workspace.is_empty());
        assert_eq!(a.workspace.window_title(), untitled);
        a.panels.add_folder(&mut a.workspace, root.clone()).unwrap();
        let title = format!("{untitled} — folder");
        assert_eq!(a.workspace.window_title(), title);
        goto(&mut a, "zh", &path);
        assert!(a.workspace.tabs[a.workspace.active].goto_preview);
        assert!(a.workspace.workbench.groups[0].sheets.is_empty());
        assert_eq!(a.workspace.window_title(), title);
        key(&mut a, "escape");
        assert_eq!(a.workspace.window_title(), title);
        goto(&mut a, "zh", &path);
        key(&mut a, "enter");
        assert_eq!(a.workspace.window_title(), "zh.md");
    }
}

fn selected_folders(a: &mut App) -> Vec<PathBuf> {
    a.scene();
    a.panels
        .side_rows
        .iter()
        .enumerate()
        .filter(|(i, _)| {
            a.panels
                .tree
                .nodes
                .get(&((3 << 40) + *i as u64))
                .is_some_and(|row| row.attributes.contains("selected"))
        })
        .map(|(_, row)| row.path.clone())
        .collect()
}

#[test]
fn r43_folder_highlight_follows_view_without_reveal_or_focus_change() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let folder = root.join("collapsed");
    fs::create_dir(&folder).unwrap();
    let origin = root.join("zh.md");
    let preview = root.join("a-preview.md");
    let hidden = folder.join("inside.md");
    for path in [&origin, &preview, &hidden] {
        fs::write(path, "# example\n").unwrap();
    }
    for i in 0..40 {
        fs::write(root.join(format!("extra-{i:02}.txt")), "extra").unwrap();
    }
    let mut a = app("en");
    a.panels.add_folder(&mut a.workspace, root.clone()).unwrap();
    until(&mut a, |a| a.panels.sidebar_has_row(&origin));
    a.workspace
        .open(FileLocation::path_only(preview.clone()))
        .unwrap();
    assert_eq!(selected_folders(&mut a), std::slice::from_ref(&preview));
    // Opening the last file leaves that row offscreen, without moving the sidebar.
    a.workspace
        .open(FileLocation::path_only(origin.clone()))
        .unwrap();
    assert!(selected_folders(&mut a).is_empty());
    assert_eq!(a.panels.side_scroll.offset, 0.);
    a.panels.side_scroll.reveal(a.panels.side_scroll.rows - 1);
    assert_eq!(selected_folders(&mut a), [origin]);
    a.panels.side_scroll.offset = 0.;
    let old_selection = a.panels.side_selected;
    // Use a third, unopened file to exercise the tabless preview path.
    let tabless = root.join("b-tabless.md");
    fs::write(&tabless, "tabless").unwrap();
    a.run_named(None, "refresh_folder_list", &json!({}));
    until(&mut a, |a| a.panels.sidebar_has_row(&tabless));
    goto(&mut a, "b-tabless", &tabless);
    assert!(a.workspace.tabs[a.workspace.active].goto_preview);
    assert_eq!(selected_folders(&mut a), [tabless]);
    assert_eq!(a.panels.side_selected, old_selection);
    assert!(!a.panels.side_focused);
    key(&mut a, "escape");
    assert!(selected_folders(&mut a).is_empty());
    goto(&mut a, "inside", &hidden);
    assert!(selected_folders(&mut a).is_empty());
    assert!(!a.panels.sidebar_has_row(&hidden));
    assert_eq!(a.panels.side_scroll.offset, 0.);
    key(&mut a, "escape");
    a.workspace.open(FileLocation::path_only(preview)).unwrap();
    a.run_named(None, "focus_side_bar", &json!({}));
    assert_eq!(selected_folders(&mut a), [root]);
    assert!(a.panels.side_focused);
}

#[test]
fn r43_open_files_defaults_off_and_restores_each_explicit_choice() {
    let mut a = app("en");
    let command = "toggle_show_open_files";
    a.run_named(None, "new_file", &json!({}));
    assert!(!a.command_checked(command, "{}"));
    assert_eq!(a.panels.folder_row_base(&a.workspace), 1);
    for enabled in [true, false] {
        a.run_named(None, command, &json!({}));
        assert_eq!(a.command_checked(command, "{}"), enabled);
        assert_eq!(
            a.panels.folder_row_base(&a.workspace),
            if enabled { 3 } else { 1 }
        );
        let (mut window, _) = a.workspace.snapshot().unwrap();
        a.panels.capture_session(&mut window);
        assert_eq!(window.sidebar_open_files, Some(enabled));
        let window = serde_json::from_value(serde_json::to_value(window).unwrap()).unwrap();
        let mut restored = app("en");
        assert!(!restored.command_checked(command, "{}"));
        restored.restore_ui(&window);
        assert_eq!(restored.command_checked(command, "{}"), enabled);
    }
}
