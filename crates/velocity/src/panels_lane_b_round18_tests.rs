//! Sidebar-only regressions; overlay and quick-panel implementations belong to Lane A.
use super::*;
use std::time::Duration;
use vel_config::settings::Layer as SettingLayer;
use vel_text::Text;
fn fixture(root: &Path) -> (Workspace, Panels) {
    let mut w = Workspace::new(Text::new(), None, None, false).unwrap();
    w.editor_mut()
        .settings
        .set(SettingLayer::View, "tree_animation_enabled", json!(false));
    let mut p = Panels::new(&w, false);
    p.add_folder(&mut w, root.into()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while p.side_rows.len() < 3 {
        p.poll(&mut w, 0..1);
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(2));
    }
    p.layout(Size {
        width: 1000.,
        height: 800.,
    });
    (w, p)
}
fn click(w: &mut Workspace, p: &mut Panels, path: &Path, double: bool) {
    p.layout(Size {
        width: 1000.,
        height: 800.,
    });
    let position = p.test_sidebar_point(w, path);
    for down in [true, false] {
        p.mouse(w, position, down, double, false, 0..1).unwrap();
    }
}
#[test]
fn round18_sidebar_preview_setting_replacement_promotion_and_session_recent_files() {
    let dir = tempfile::tempdir().unwrap();
    let first = dir.path().canonicalize().unwrap().join("a.txt");
    let second = dir.path().canonicalize().unwrap().join("b.txt");
    std::fs::write(&first, "first").unwrap();
    std::fs::write(&second, "second").unwrap();
    let (mut w, mut p) = fixture(dir.path());
    let origin = w.tabs[w.active].sheet;
    w.editor_mut()
        .settings
        .set(SettingLayer::View, "preview_on_click", json!(false));
    click(&mut w, &mut p, &first, false);
    assert_eq!(w.tabs[w.active].sheet, origin);
    assert_eq!(p.side_selected, 1);
    click(&mut w, &mut p, &first, true);
    assert_eq!(w.tabs[w.active].path.as_ref(), Some(&first));
    assert!(!w.tabs[w.active].transient);
    assert!(w.recent_files.contains(&first));
    w.editor_mut()
        .settings
        .set(SettingLayer::View, "preview_on_click", json!(true));
    click(&mut w, &mut p, &second, false);
    assert!(w.tabs[w.active].transient);
    assert!(!w.recent_files.contains(&second));
    let preview = w.tabs[w.active].sheet;
    let (snapshot, documents) = w.snapshot().unwrap();
    assert!(!snapshot.sheets.iter().any(|s| s.id == preview));
    assert!(
        !snapshot
            .workbench
            .groups
            .iter()
            .any(|g| g.sheets.contains(&preview))
    );
    assert!(!documents.iter().any(|d| d.path.as_ref() == Some(&second)));
    click(&mut w, &mut p, &first, false);
    assert!(w.tabs.iter().any(|t| t.sheet == preview));
    assert!(
        !w.tabs[w.active].transient,
        "permanent tabs must not become previews"
    );
    click(&mut w, &mut p, &second, true);
    assert!(!w.tabs[w.active].transient);
    assert!(w.recent_files.contains(&second));
    w.close(w.active).unwrap();
    w.editor_mut()
        .settings
        .set(SettingLayer::View, "preview_on_click", json!("only_left"));
    click(&mut w, &mut p, &second, false);
    assert!(w.tabs[w.active].transient);
    w.editor_mut()
        .run("insert", &json!({"characters":"dirty"}))
        .unwrap();
    w.sync_shared();
    assert!(!w.tabs[w.active].transient);
    assert!(w.recent_files.contains(&second));
    let dirty = w.tabs[w.active].sheet;
    click(&mut w, &mut p, &first, false);
    assert!(w.tabs.iter().any(|t| t.sheet == dirty && t.dirty()));
    w.close(w.sheet_index(dirty).unwrap()).unwrap();
    click(&mut w, &mut p, &second, false);
    assert!(w.tabs[w.active].transient);
    w.save(w.active, None).unwrap();
    assert!(!w.tabs[w.active].transient);
}
#[test]
fn round18_sidebar_next_preview_replaces_only_clean_transient_tabs() {
    let dir = tempfile::tempdir().unwrap();
    let paths = [
        dir.path().canonicalize().unwrap().join("a.txt"),
        dir.path().canonicalize().unwrap().join("b.txt"),
        dir.path().canonicalize().unwrap().join("c.txt"),
    ];
    for path in &paths {
        std::fs::write(path, "text").unwrap();
    }
    let (mut w, mut p) = fixture(dir.path());
    click(&mut w, &mut p, &paths[0], false);
    let preview = w.tabs[w.active].sheet;
    click(&mut w, &mut p, &paths[1], false);
    assert!(w.sheet_index(preview).is_none());
    assert!(!w.recent_files.contains(&paths[0]));
    click(&mut w, &mut p, &paths[1], true);
    let pinned = w.tabs[w.active].sheet;
    click(&mut w, &mut p, &paths[2], false);
    assert!(w.sheet_index(pinned).is_some());
    assert!(!w.tabs[w.sheet_index(pinned).unwrap()].transient);
}
#[test]
fn round18_tree_motion_expand_collapse_repetition_disabled_and_reduce_motion() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().canonicalize().unwrap().join("folder")).unwrap();
    std::fs::write(dir.path().canonicalize().unwrap().join("other.txt"), "text").unwrap();
    std::fs::write(
        dir.path().canonicalize().unwrap().join("folder/child.txt"),
        "text",
    )
    .unwrap();
    let (mut w, mut p) = fixture(dir.path());
    let folder = dir.path().canonicalize().unwrap().join("folder");
    for (enabled, reduce) in [(false, false), (true, false), (true, true)] {
        w.editor_mut()
            .settings
            .set(SettingLayer::View, "tree_animation_enabled", json!(enabled));
        w.themes.ui.borrow_mut().set_accessibility_preferences(
            vel_platform::accessibility::Preferences {
                reduce_motion: reduce,
                increase_contrast: false,
            },
        );
        for _ in 0..4 {
            p.side_selected = p.side_rows.iter().position(|r| r.path == folder).unwrap();
            p.activate_side(&mut w, false).unwrap();
            let deadline = Instant::now() + Duration::from_secs(3);
            while p.loading.contains(&folder) {
                p.poll(&mut w, 0..1);
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(1));
            }
            assert_eq!(p.tree_motion.is_some(), enabled && !reduce);
            if let Some((start, _)) = &p.tree_motion {
                let end = *start + Duration::from_secs(1);
                for i in 0..p.side_rows.len() {
                    assert_eq!(p.tree_row_position(i, end), i as f64);
                }
            }
            p.paint(&w, &mut Scene::default());
        }
    }
}
#[test]
fn round18_index_files_toggle_leaves_searchable_file_lists_including_gitignored_target() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().canonicalize().unwrap().join("target")).unwrap();
    std::fs::write(
        dir.path().canonicalize().unwrap().join("a.rs"),
        "fn a() {}\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().canonicalize().unwrap().join("target/ignored.rs"),
        "fn ignored() {}\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().canonicalize().unwrap().join(".gitignore"),
        "target/\n",
    )
    .unwrap();
    let (mut w, mut p) = fixture(dir.path());
    let deadline = Instant::now() + Duration::from_secs(5);
    while !p
        .file_paths
        .iter()
        .any(|f| f.ends_with("target/ignored.rs"))
    {
        p.poll(&mut w, 0..1);
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(2));
    }
    let files = p.file_paths.clone();
    for enabled in [false, true, false] {
        // Indexing is application-global; view-local preferences intentionally do not restart it.
        w.settings
            .set(SettingLayer::User, "index_files", json!(enabled));
        p.poll(&mut w, 0..1);
        if !enabled {
            assert!(p.intel.projects.is_empty());
        }
        assert_eq!(p.file_paths, files);
    }
}
