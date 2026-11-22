use super::*;
use crate::cli::FileLocation;
use crate::panels::Overlay;
use std::{cell::RefCell, fs, path::Path, rc::Rc};
use vel_buffer::{Region, Selection};
use vel_config::keymap::Key;
use vel_platform::{Modifiers, accessibility::Role};
use vel_text::Text;

const ORIGIN: &str = "origin.txt";
const TARGET: &str = "target.txt";

fn fixture(root: &Path) -> App {
    fs::write(root.join(ORIGIN), "original editor text\n".repeat(240)).unwrap();
    fs::write(root.join(TARGET), "preview body sentinel\n".repeat(240)).unwrap();
    let now = Instant::now();
    let options = Options::default();
    let mut workspace = Workspace::new(Text::new(), None, None, false).unwrap();
    workspace
        .open(FileLocation::path_only(root.join(ORIGIN)))
        .unwrap();
    let mut app = App::new(
        options.clone(),
        Clock::new(now, std::time::SystemTime::now()),
        Reporter::for_options(&options, now).unwrap(),
        workspace,
        Arc::new(AtomicBool::new(false)),
        Rc::new(RefCell::new(crate::session::Sessions::new(None))),
    );
    app.panels
        .add_folder(&mut app.workspace, root.to_owned())
        .unwrap();
    app
}

fn key(app: &mut App, chord: &str) {
    assert!(
        app.dispatch_key_binding(None, &Key::parse(chord).unwrap(), Instant::now(), 0..0),
        "{chord}"
    );
}

fn click(app: &mut App, point: Point) {
    for kind in [MouseKind::Down, MouseKind::Up] {
        app.handle_mouse_event(
            None,
            MouseEvent {
                button: 0,
                kind,
                position: point,
                click_count: 1,
                modifiers: Modifiers::default(),
                timestamp: 0.,
            },
        );
    }
}

fn goto(app: &mut App, query: &str, file: &str) {
    app.run_named(
        None,
        "show_overlay",
        &json!({"overlay":"goto", "text":query}),
    );
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        app.workspace.poll();
        app.panels.poll(&mut app.workspace, 0..0);
        if !app.panels.ranking_pending()
            && app.workspace.tabs[app.workspace.active]
                .path
                .as_deref()
                .and_then(Path::file_name)
                .is_some_and(|name| name == file)
        {
            break;
        }
        assert!(Instant::now() < deadline, "Goto did not preview {query}");
        std::thread::park_timeout(Duration::from_millis(1));
    }
}

#[test]
fn r41_tabless_scene_and_commit_preserve_view() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let mut app = fixture(&root);
    let other = root.join("other.txt");
    fs::write(&other, "another open file").unwrap();
    app.workspace.open(FileLocation::path_only(other)).unwrap();
    app.workspace.select(0); // Open to the right edge, not immediately after this tab.
    let original_tabs = app.workspace.workbench.groups[0].sheets.clone();
    let origin = original_tabs[0];
    app.scene();
    goto(&mut app, "target:133:3", TARGET);
    let scene = app.scene();
    let preview = &app.workspace.tabs[app.workspace.active];
    let sheet = preview.sheet;
    assert!(preview.goto_preview);
    assert_eq!(app.workspace.workbench.groups[0].sheets, original_tabs);
    assert_eq!(
        app.tab_hits.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
        original_tabs
    );
    assert!(!app.tab_close_hits.iter().any(|(id, _)| *id == sheet));
    assert!(
        app.panels.tree.nodes[&(100 + origin)]
            .attributes
            .contains("selected")
    );
    assert!(!app.panels.tree.nodes.contains_key(&(100 + sheet)));
    assert!(app.sheet_bounds.contains_key(&sheet));
    let tab_bar = app.tab_bars[0].1;
    let runs = scene.layers.iter().flat_map(|layer| &layer.text);
    assert!(
        runs.clone()
            .any(|run| run.text.contains("preview body sentinel"))
    );
    assert!(!runs.clone().any(|run| run.text.contains(TARGET)
        && run.baseline as f64 >= tab_bar.y
        && (run.baseline as f64) < tab_bar.y + tab_bar.height));
    assert!(
        !app.accessible_tree()
            .nodes
            .iter()
            .any(|n| { matches!(n.role, Role::Tab | Role::TextArea) && n.label.contains(TARGET) })
    );
    assert_eq!(app.workspace.window_title(), ORIGIN);
    assert_eq!(
        app.workspace
            .editor()
            .text
            .byte_to_line(app.workspace.editor().caret),
        132
    );
    assert!(app.view.scroll_y > 0.);
    let at = app.workspace.editor().caret;
    let selection = Selection::from_regions([Region::new(at + 2, at), Region::caret(at + 7)]);
    app.workspace.editor_mut().set_selection(selection.clone());
    app.view.scroll_y += 64.;
    let scroll = (app.view.scroll_x, app.view.scroll_y);
    key(&mut app, "enter");
    assert!(app.panels.overlay.is_none());
    let mut committed_tabs = original_tabs;
    committed_tabs.push(sheet);
    assert_eq!(app.workspace.workbench.groups[0].sheets, committed_tabs);
    let tab = &app.workspace.tabs[app.workspace.active];
    assert_eq!(tab.sheet, sheet);
    assert!(!tab.goto_preview && !tab.transient);
    assert_eq!(
        app.workspace.editor().regions(),
        selection.iter().collect::<Vec<_>>()
    );
    assert_eq!((app.view.scroll_x, app.view.scroll_y), scroll);
    assert_eq!(app.workspace.recent_files[0], root.join(TARGET));
    assert_eq!(app.workspace.goto_recent_files[0], root.join(TARGET));
    app.scene();
    assert!(
        app.accessible_tree()
            .nodes
            .iter()
            .any(|n| n.role == Role::Tab && n.label == TARGET)
    );
}

#[test]
fn r41_cancel_restores_origin_and_open_tab_navigation() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let mut app = fixture(&root);
    let existing = root.join("existing.txt");
    fs::write(&existing, "already open\n".repeat(120)).unwrap();
    app.workspace
        .open(FileLocation::path_only(existing))
        .unwrap();
    let existing = app.workspace.tabs[app.workspace.active].sheet;
    app.workspace.select(0);
    app.scene();
    let origin = app.workspace.tabs[0].sheet;
    let before = app.workspace.workbench.groups[0].clone();
    let caret = app.workspace.editor().text.line_to_byte(100);
    let selection =
        Selection::from_regions([Region::new(caret + 5, caret), Region::caret(caret + 15)]);
    app.workspace.editor_mut().set_selection(selection.clone());
    let scroll = (0., app.view.line_height * 79.);
    app.view.scroll_y = scroll.1;
    let recents = (
        app.workspace.recent_files.clone(),
        app.workspace.goto_recent_files.clone(),
    );

    for (query, file, exit) in [
        ("existing:80", "existing.txt", "escape"),
        ("target:133", TARGET, "escape"),
        ("target:133", TARGET, "click"),
        ("target:133", TARGET, "blur"),
    ] {
        app.set_window_focused(true);
        goto(&mut app, query, file);
        app.scene();
        let sheet = app.workspace.tabs[app.workspace.active].sheet;
        if file == "existing.txt" {
            assert_eq!(sheet, existing);
            assert!(
                app.panels.tree.nodes[&(100 + existing)]
                    .attributes
                    .contains("selected")
            );
            assert_eq!(app.workspace.tabs.len(), 2);
        } else {
            let (snapshot, docs) = app.capture_session_window().unwrap();
            assert_eq!(snapshot.workbench.groups[0].sheets, before.sheets);
            assert_eq!(snapshot.workbench.groups[0].focused, before.focused);
            assert!(!snapshot.sheets.iter().any(|s| s.id == sheet));
            assert!(
                !docs
                    .iter()
                    .any(|d| d.path.as_deref() == Some(root.join(TARGET).as_path()))
            );
            assert!(app.workspace.workbench.group_of(sheet).is_none());
        }
        match exit {
            "escape" => key(&mut app, "escape"),
            "click" => {
                let point = Point {
                    x: 1.,
                    y: app.window_size.height - 2.,
                };
                click(&mut app, point);
            }
            _ => app.set_window_focused(false),
        }
        assert!(app.panels.overlay.is_none());
        assert_eq!(app.workspace.tabs[app.workspace.active].sheet, origin);
        assert_eq!(
            app.workspace.editor().regions(),
            selection.iter().collect::<Vec<_>>()
        );
        assert_eq!((app.view.scroll_x, app.view.scroll_y), scroll);
        assert!(app.workspace.closed_files.is_empty());
        assert_eq!(app.workspace.recent_files, recents.0);
        assert_eq!(app.workspace.goto_recent_files, recents.1);
        assert_eq!(app.workspace.tabs.len(), 2);
        let after = &app.workspace.workbench.groups[0];
        assert_eq!(after.sheets, before.sheets);
        assert_eq!(after.selected, before.selected);
        assert_eq!(after.focused, before.focused);
        assert_eq!(after.anchor, before.anchor);
        assert_eq!(after.mru, before.mru);
    }
}

#[test]
fn r41_sidebar_transient_tab_and_goto_click_acceptance() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let mut app = fixture(&root);
    let side = root.join("sidebar.txt");
    fs::write(&side, "sidebar preview").unwrap();
    app.workspace
        .open_from_sidebar(FileLocation::path_only(side), false)
        .unwrap();
    app.scene();
    let sidebar = app.workspace.tabs[app.workspace.active].sheet;
    assert!(
        app.panels.tree.nodes[&(100 + sidebar)]
            .attributes
            .contains("transient")
    );
    goto(&mut app, "target", TARGET);
    app.scene();
    assert!(
        app.panels.tree.nodes[&(100 + sidebar)]
            .attributes
            .contains("transient")
    );
    let point = app
        .accessible_tree()
        .nodes
        .iter()
        .find(|n| n.role == Role::Row && n.label.contains(TARGET))
        .and_then(|n| n.activation)
        .unwrap()
        .0;
    click(&mut app, point);
    assert_ne!(app.panels.overlay, Some(Overlay::Goto));
    let tab = &app.workspace.tabs[app.workspace.active];
    assert_eq!(tab.path.as_deref(), Some(root.join(TARGET).as_path()));
    assert!(!tab.goto_preview && !tab.transient);
    assert_eq!(
        app.workspace.workbench.groups[0].sheets.last(),
        Some(&tab.sheet)
    );
    assert!(app.workspace.tabs[app.workspace.sheet_index(sidebar).unwrap()].transient);
}

/// U01 and U04 together, as Sublime 4215 shows them: a folder window starts tabless, Goto
/// previews without a tab, Escape returns to the tabless input, and Return opens the file
/// as the only tab (the untouched input is reused, not kept as an "untitled" tab).
#[test]
fn r41_goto_preview_in_a_tabless_folder_window() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    fs::write(root.join(TARGET), "preview body sentinel\n".repeat(40)).unwrap();
    let now = Instant::now();
    let options = Options::default();
    let workspace =
        Workspace::new_for_startup(Text::new(), None, None, false, &mut Default::default())
            .unwrap();
    let mut app = App::new(
        options.clone(),
        Clock::new(now, std::time::SystemTime::now()),
        Reporter::for_options(&options, now).unwrap(),
        workspace,
        Arc::new(AtomicBool::new(false)),
        Rc::new(RefCell::new(crate::session::Sessions::new(None))),
    );
    app.panels
        .add_folder(&mut app.workspace, root.to_owned())
        .unwrap();
    app.scene();
    assert!(app.workspace.is_empty() && app.tab_hits.is_empty());

    goto(&mut app, "target", TARGET);
    let preview = app.workspace.tabs[app.workspace.active].sheet;
    let scene = app.scene();
    assert!(app.tab_hits.is_empty(), "the preview must not add a tab");
    assert!(app.sheet_bounds.contains_key(&preview));
    assert!(
        scene
            .layers
            .iter()
            .flat_map(|layer| &layer.text)
            .any(|run| run.text.contains("preview body sentinel"))
    );
    key(&mut app, "escape");
    app.scene();
    assert!(app.workspace.is_empty() && app.tab_hits.is_empty());
    assert!(app.workspace.editor().text.is_empty());

    goto(&mut app, "target", TARGET);
    key(&mut app, "enter");
    app.scene();
    assert!(!app.workspace.is_empty());
    let sheets = &app.workspace.workbench.groups[0].sheets;
    assert_eq!(sheets.len(), 1, "only the opened file has a tab");
    let tab = &app.workspace.tabs[app.workspace.active];
    assert_eq!(tab.sheet, sheets[0]);
    assert_eq!(tab.path.as_deref(), Some(root.join(TARGET).as_path()));
    assert_eq!(app.tab_hits.len(), 1);
}
