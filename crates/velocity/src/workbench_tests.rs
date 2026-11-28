//! Integration tests run entirely through device-free controllers.
use crate::{cli::FileLocation, workspace::Workspace};
use vel_buffer::{Region, Selection};
use vel_config::{json, settings::Layer};
use vel_text::Text;
#[test]
fn shared_buffer_views_have_independent_selections_and_global_undo_after_owner_close() {
    let mut w = Workspace::new(Text::from("one\ntwo\n"), None, None, false).unwrap();
    let original = w.tabs[0].sheet;
    w.editor_mut().set_caret(4, false);
    w.tabs[0].scroll = (0., 80.);
    w.clone_file().unwrap();
    let cloned = w.active;
    let doc = w.tabs[cloned].document;
    w.editor_mut().set_caret(0, false);
    w.editor_mut()
        .run("insert", &json!({"characters":"hello "}))
        .unwrap();
    w.sync_shared();
    assert_eq!(w.tabs[0].editor.text.to_string(), "hello one\ntwo\n");
    let original_index = w.sheet_index(original).unwrap();
    w.select(original_index);
    assert_eq!(w.editor().buffer.id(), doc);
    assert_eq!(w.editor().caret, 10);
    assert_eq!(w.tabs[0].scroll, (0., 80.));
    w.editor_mut().run("undo", &json!({})).unwrap();
    w.select(cloned);
    assert_eq!(w.editor().text.to_string(), "one\ntwo\n");
    w.editor_mut().run("redo", &json!({})).unwrap();
    w.close(cloned).unwrap();
    assert_eq!(w.editor().buffer.id(), doc);
    w.editor_mut().run("undo", &json!({})).unwrap();
    assert_eq!(w.editor().text.to_string(), "one\ntwo\n");
}
#[test]
fn grouped_typing_survives_four_view_scene_transfers_and_session_restore() {
    let mut w = Workspace::new(Text::new(), None, None, false).unwrap();
    crate::workbench::configure_groups(&mut w, 4).unwrap();
    let original = w.active;
    for c in ["h", "e", "l", "l", "o"] {
        w.editor_mut()
            .run("insert", &json!({"characters":c}))
            .unwrap();
        for index in 0..w.tabs.len() {
            w.prepare_sheet(index).unwrap();
        }
        w.prepare_sheet(original).unwrap();
    }
    let (window, documents) = w.snapshot().unwrap();
    let session = vel_session::Session {
        documents,
        windows: vec![window],
        ..Default::default()
    };
    session.validate().unwrap();
    let bytes = serde_json::to_vec(&session).unwrap();
    let session: vel_session::Session = serde_json::from_slice(&bytes).unwrap();
    let mut restored = Workspace::new(Text::new(), None, None, false).unwrap();
    restored
        .restore_window(&session.windows[0], &session.documents)
        .unwrap();
    assert_eq!(restored.tabs.len(), 4);
    assert_eq!(restored.editor().text.to_string(), "hello");
    restored.editor_mut().run("undo", &json!({})).unwrap();
    assert_eq!(restored.editor().text.to_string(), "");
    restored.editor_mut().run("redo", &json!({})).unwrap();
    assert_eq!(restored.editor().text.to_string(), "hello");
}
#[test]
fn lazy_restore_keeps_hidden_file_unopened_and_clones_attach_on_demand() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("lazy.txt");
    std::fs::write(&path, "lazy content").unwrap();
    let mut w = Workspace::new(Text::from("unsaved"), None, None, false).unwrap();
    let first = w.tabs[0].sheet;
    w.clone_file().unwrap();
    w.open(FileLocation {
        path: path.clone(),
        line: 1,
        column: 1,
    })
    .unwrap();
    w.select(w.sheet_index(first).unwrap());
    let (window, documents) = w.snapshot().unwrap();
    std::fs::remove_file(&path).unwrap();
    let mut restored = Workspace::new(Text::new(), None, None, false).unwrap();
    restored.restore_window(&window, &documents).unwrap();
    assert_eq!(restored.tabs.iter().filter(|t| t.lazy.is_some()).count(), 2);
    let clone = 1;
    restored.select(clone);
    assert_eq!(restored.editor().text.to_string(), "unsaved");
    let (window, docs) = restored.snapshot().unwrap();
    vel_session::Session {
        windows: vec![window],
        documents: docs,
        ..Default::default()
    }
    .validate()
    .unwrap();
    restored.prepare_sheet(2).unwrap();
    assert!(restored.tabs[2].deleted);
    assert!(!restored.tabs[2].dirty());
    assert!(restored.tabs[2].load_failed);
    assert!(restored.tabs[2].editor.read_only);
    assert!(restored.tabs[2].lazy.is_none());
    assert_eq!(restored.tabs[0].editor.text.to_string(), "unsaved");
}
#[test]
fn session_selections_folds_syntax_and_changed_disk_keep_edits_drop_history() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("changed.txt");
    std::fs::write(&path, "saved line\n").unwrap();
    let mut w = Workspace::new(Text::new(), None, None, false).unwrap();
    w.open(FileLocation {
        path: path.clone(),
        line: 1,
        column: 1,
    })
    .unwrap();
    w.editor_mut().run("paste", &json!({})).unwrap();
    w.editor_mut()
        .run("insert", &json!({"characters":"local "}))
        .unwrap();
    w.editor_mut()
        .set_selection(Selection::from_regions([Region::new(2, 7)]));
    let e = w.editor_mut();
    e.buffer
        .add_regions(e.id, "velocity.folds", vec![Region::new(1, 5)], true);
    w.set_file_type("Plain Text").unwrap();
    w.tabs[0].scroll = (2., 12.);
    let (window, documents) = w.snapshot().unwrap();
    std::fs::write(&path, "external longer content\n").unwrap();
    let mut restored = Workspace::new(Text::new(), None, None, false).unwrap();
    restored.restore_window(&window, &documents).unwrap();
    assert_eq!(restored.editor().text.to_string(), "local saved line\n");
    assert!(restored.tabs[0].dirty());
    assert_eq!(restored.editor().regions(), vec![Region::new(2, 7)]);
    // Restoring a selection endpoint inside a fold reveals that fold (round 16).
    assert!(restored.editor().folded().is_empty());
    assert_eq!(restored.tabs[0].scroll, (2., 12.));
    assert!(restored.editor().settings.get("syntax").is_some());
    let before = restored.editor().text.clone();
    restored.editor_mut().run("undo", &json!({})).unwrap();
    assert_eq!(restored.editor().text, before);
}
#[test]
fn distraction_free_layer_restores_editor_preferences() {
    let mut w = Workspace::new(Text::new(), None, None, false).unwrap();
    assert!(w.editor().preferences.line_numbers);
    w.workbench_action("toggle_distraction_free", &json!({}))
        .unwrap();
    assert!(!w.editor().preferences.line_numbers);
    assert_eq!(w.editor().settings.usize("wrap_width", 0), 80);
    w.editor_mut()
        .settings
        .set(Layer::View, "tab_size", json!(8));
    w.workbench_action("toggle_distraction_free", &json!({}))
        .unwrap();
    assert!(w.editor().preferences.line_numbers);
    assert_eq!(w.editor().preferences.tab_size, 8);
}
#[test]
fn launch_with_nonexistent_readonly_and_corrupt_data_roots() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("brand-new/profile");
    assert!(!missing.exists());
    let w = Workspace::new(Text::new(), Some(missing.clone()), None, false).unwrap();
    // Optional bundled references are console diagnostics, not launch errors.
    assert!(w.messages.is_empty(), "{:?}", w.messages);
    assert!(missing.join("Packages/User").is_dir());
    let corrupt = dir.path().join("data-is-file");
    std::fs::write(&corrupt, "bad directory").unwrap();
    Workspace::new(Text::new(), Some(corrupt), None, false).unwrap();
    let bad = missing.join("Packages/User/Preferences.sublime-settings");
    std::fs::write(&bad, "{ corrupt").unwrap();
    let w = Workspace::new(Text::new(), Some(missing), None, false).unwrap();
    assert!(w.messages.is_empty());
    assert!(
        w.resource_reports
            .iter()
            .any(|message| message.contains("Preferences.sublime-settings:1:")),
        "{:?}",
        w.resource_reports
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let root = dir.path().join("readonly");
        std::fs::create_dir(&root).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o555)).unwrap();
        let result = Workspace::new(Text::new(), Some(root.clone()), None, false);
        std::fs::set_permissions(root, std::fs::Permissions::from_mode(0o755)).unwrap();
        result.unwrap();
    }
}
#[test]
fn project_save_as_keeps_rules_and_unknown_keys_and_loads_inline_builds() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("original.sublime-project");
    std::fs::write(&project,r#"{"folders":[{"path":".","name":"Source","file_exclude_patterns":["*.bak"],"follow_symlinks":true}],"build_systems":[{"name":"Unit","cmd":["true"]}],"custom":42}"#).unwrap();
    let mut w = Workspace::new(Text::new(), None, Some(project), false).unwrap();
    assert!(w.build_systems.iter().any(|b| b.name == "Unit"));
    let new = dir.path().join("new/tree/project.sublime-project");
    w.save_project_as(new.clone()).unwrap();
    let model = vel_session::project::Project::load(&new).unwrap();
    assert_eq!(model.extra["custom"], 42);
    assert_eq!(model.folders[0].file_exclude_patterns, vec!["*.bak"]);
    assert_eq!(model.folders[0].follow_symlinks, Some(true));
    w.close_project().unwrap();
    assert!(w.folders.is_empty());
    assert!(w.project_path().is_none());
}
#[test]
#[cfg(unix)]
fn streamed_build_output_result_navigation_and_output_double_click() {
    use std::time::{Duration, Instant};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("error.txt");
    std::fs::write(&path, "one\ntwo\nthree").unwrap();
    let mut w = Workspace::new(Text::new(), None, None, false).unwrap();
    let mut panels = crate::panels::Panels::new(&w, false);
    let mut builds = crate::build::Builds::default();
    builds.start(&w,&mut panels,&json!({"cmd":["/bin/sh","-c","printf 'error.txt:2:2: fixture error\\n'"],"working_dir":dir.path(),"file_regex":"^([^:]+):([0-9]+):([0-9]+): (.*)$","quiet":true})).unwrap();
    let at = Instant::now();
    while builds.task.is_some() {
        builds.poll(&mut panels);
        assert!(
            at.elapsed() < crate::test_timing::HANG_GUARD,
            "build did not finish"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(builds.diagnostics.len(), 1);
    assert_eq!(panels.output, "error.txt:2:2: fixture error\n");
    assert!(
        !panels.output.contains("[Finished"),
        "quiet suppresses the finish summary"
    );
    builds.navigate(&mut w, true).unwrap();
    assert_eq!(
        w.tabs[w.active].path.as_ref(),
        Some(&std::fs::canonicalize(&path).unwrap())
    );
    assert_eq!(w.editor().caret, 5);
    panels.layout(vel_platform::Size {
        width: 1200.,
        height: 800.,
    });
    let node = &panels.tree.nodes[&2];
    let p = vel_platform::Point {
        x: node.rect.x + 15.,
        y: node.rect.y + 35.,
    };
    panels.mouse(&mut w, p, true, true, false, 0..20).unwrap();
    assert_eq!(w.editor().caret, 5);
    assert!(
        !panels
            .key(
                &mut w,
                &vel_config::keymap::Key::parse("f4").unwrap(),
                0..20
            )
            .unwrap()
            .0
    );
    assert!(
        !panels
            .key(
                &mut w,
                &vel_config::keymap::Key::parse("shift+f4").unwrap(),
                0..20
            )
            .unwrap()
            .0
    );
}

#[test]
fn clone_wrap_layouts_receive_exact_edits_and_external_reload_keeps_shared_views() {
    let mut w =
        Workspace::new(Text::from("short words\n".repeat(1000)), None, None, false).unwrap();
    crate::workbench::configure_groups(&mut w, 4).unwrap();
    let original = w.active;
    let mut views = (0..4)
        .map(|_| {
            crate::view::View::new(
                vel_platform::Size {
                    width: 300.,
                    height: 300.,
                },
                7.8,
                12.,
            )
        })
        .collect::<Vec<_>>();
    for (i, v) in views.iter_mut().enumerate() {
        w.prepare_sheet(i).unwrap();
        v.scene(&w.tabs[i].editor);
    }
    w.prepare_sheet(original).unwrap();
    w.editor_mut()
        .run("insert", &json!({"characters":"x"}))
        .unwrap();
    for (i, v) in views.iter_mut().enumerate() {
        w.prepare_sheet(i).unwrap();
        v.scene(&w.tabs[i].editor);
        assert_eq!(w.tabs[i].editor.layout.borrow().rewrapped, 1);
    }
    w.prepare_sheet(original).unwrap();
    w.tabs[original].install_restored(vel_fileio::Loaded {
        disk_content: None,
        encoding_chosen: false,
        unchanged: false,
        warning: None,
        text: Text::from("external\n"),
        format: Default::default(),
        source_bytes: 9,
    });
    w.reconcile_installs();
    let document = w.tabs[original].document;
    for i in 0..4 {
        w.select(i);
        assert_eq!(w.editor().buffer.id(), document);
        assert_eq!(w.editor().text.to_string(), "external\n");
    }
}

#[test]
fn workspace_autosave_does_not_reload_project_settings_from_parent_directory_changes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("p.sublime-project");
    std::fs::write(&path, "{}").unwrap();
    let mut w = Workspace::new(Text::new(), None, Some(path.clone()), false).unwrap();
    let revision = w.editor().settings.revision();
    std::fs::write(path.with_extension("sublime-workspace"), "{}").unwrap();
    w.wait_resources();
    assert_eq!(w.editor().settings.revision(), revision);
}

#[test]
fn query_history_accumulates_bounded_values_and_restores_an_empty_replacement() {
    let mut w = Workspace::new(Text::new(), None, None, false).unwrap();
    let mut panels = crate::panels::Panels::new(&w, false);
    let mut window = vel_session::Window::default();
    panels.query.set("first");
    panels.replace.set("replacement");
    panels.capture_session(&mut window);
    assert!(window.find_history.is_empty());
    assert!(window.replace_history.is_empty());
    panels
        .action(&mut w, "replace_all", &json!({}), 0..0)
        .unwrap();
    panels.query.set("second");
    panels.replace.set("");
    panels
        .action(&mut w, "replace_all", &json!({}), 0..0)
        .unwrap();
    panels.capture_session(&mut window);
    assert_eq!(window.find_history, vec!["first", "second"]);
    assert_eq!(window.replace_history, vec!["replacement", ""]);
    for i in 0..105 {
        panels.query.set(&format!("query{i}"));
        panels
            .action(&mut w, "find_next", &json!({}), 0..0)
            .unwrap();
        panels.capture_session(&mut window);
    }
    assert_eq!(window.find_history.len(), 100);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("new/Sessions/state");
    vel_session::save(
        &path,
        &vel_session::Session {
            windows: vec![window],
            ..Default::default()
        },
    )
    .unwrap();
    let restored = vel_session::load(&path);
    let mut panels = crate::panels::Panels::new(&w, false);
    panels.restore_session(&w, &restored.session.windows[0]);
    assert_eq!(panels.query.value(), "query104");
    assert_eq!(panels.replace.value(), "");
}

#[test]
fn native_history_continues_typing_group_after_application_restore() {
    let mut live = Workspace::new(Text::new(), None, None, false).unwrap();
    for s in ["h", "e"] {
        live.editor_mut()
            .run("insert", &json!({"characters":s}))
            .unwrap();
    }
    let (window, documents) = live.snapshot().unwrap();
    let session = vel_session::Session {
        windows: vec![window],
        documents,
        ..Default::default()
    };
    let session: vel_session::Session =
        serde_json::from_slice(&serde_json::to_vec(&session).unwrap()).unwrap();
    let mut restored = Workspace::new(Text::new(), None, None, false).unwrap();
    restored
        .restore_window(&session.windows[0], &session.documents)
        .unwrap();
    for s in ["l", "l", "o"] {
        restored
            .editor_mut()
            .run("insert", &json!({"characters":s}))
            .unwrap();
    }
    restored.editor_mut().run("undo", &json!({})).unwrap();
    assert_eq!(restored.editor().text, "");
    assert!(!restored.editor().buffer.is_dirty());
}

#[test]
fn lazy_clone_snapshot_uses_the_actual_owner_after_several_activation_transfers() {
    let mut w = Workspace::new(Text::from("shared"), None, None, false).unwrap();
    for _ in 0..3 {
        w.clone_file().unwrap();
    }
    let (window, documents) = w.snapshot().unwrap();
    let mut restored = Workspace::new(Text::new(), None, None, false).unwrap();
    restored.restore_window(&window, &documents).unwrap();
    restored.select(0);
    restored.select(1);
    assert!(restored.tabs[2].lazy.is_some());
    restored
        .editor_mut()
        .run("insert", &json!({"characters":"updated"}))
        .unwrap();
    let (window, documents) = restored.snapshot().unwrap();
    let session = vel_session::Session {
        windows: vec![window],
        documents,
        ..Default::default()
    };
    session.validate().unwrap();
    let wire = serde_json::to_vec(&session).unwrap();
    let session: vel_session::Session = serde_json::from_slice(&wire).unwrap();
    let mut again = Workspace::new(Text::new(), None, None, false).unwrap();
    again
        .restore_window(&session.windows[0], &session.documents)
        .unwrap();
    again.select(2);
    assert_eq!(again.editor().text, "updatedshared");
    again.editor_mut().run("undo", &json!({})).unwrap();
    assert_eq!(again.editor().text, "shared");
}

#[test]
fn review15_lazy_labels_and_first_load_indentation_survive_appearance_setup() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("lazy.rs");
    let new = dir.path().join("new.rs");
    std::fs::write(&file, "fn main() {\n  one();\n  two();\n}\n").unwrap();
    let mut live = Workspace::new(Text::from("anchor"), None, None, false).unwrap();
    for path in [&file, &new] {
        live.open(FileLocation {
            path: path.into(),
            line: 1,
            column: 1,
        })
        .unwrap();
    }
    live.select(0);
    let (window, documents) = live.snapshot().unwrap();
    let mut restored = Workspace::new(Text::new(), None, None, false).unwrap();
    restored.restore_window(&window, &documents).unwrap();
    assert!(restored.tabs[1].lazy.is_some());
    assert!(!restored.tabs[1].label().contains("[new]"));
    assert!(restored.tabs[2].label().contains("[deleted]"));
    restored.set_appearance(true).unwrap();
    restored.set_appearance(false).unwrap();
    assert!(restored.tabs[1].lazy.is_some());
    restored.prepare_sheet(1).unwrap();
    assert_eq!(restored.tabs[1].editor.settings.usize("tab_size", 0), 2);
    restored.tabs[1]
        .editor
        .settings
        .set(Layer::View, "tab_size", json!(8));
    restored.set_appearance(true).unwrap();
    assert_eq!(restored.tabs[1].editor.settings.usize("tab_size", 0), 8);
}

#[cfg(unix)]
#[test]
fn review15_dirty_lazy_history_never_reads_unreadable_or_invalid_disk_text() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("unsaved.txt");
    for failure in ["permissions", "decode", "missing"] {
        std::fs::write(&file, "disk text").unwrap();
        let mut live = Workspace::new(Text::from("anchor"), None, None, false).unwrap();
        live.open(FileLocation {
            path: file.clone(),
            line: 1,
            column: 1,
        })
        .unwrap();
        live.editor_mut()
            .run("insert", &json!({"characters":"UNSAVED "}))
            .unwrap();
        live.clone_file().unwrap();
        let expected = live.editor().text.clone();
        live.select(0);
        let (window, documents) = live.snapshot().unwrap();
        let wire = serde_json::to_vec(&vel_session::Session {
            windows: vec![window],
            documents,
            ..Default::default()
        })
        .unwrap();
        let session: vel_session::Session = serde_json::from_slice(&wire).unwrap();
        match failure {
            "permissions" => {
                std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o0)).unwrap()
            }
            "decode" => {
                std::fs::write(&file, b"\0invalid disk text").unwrap();
                assert!(vel_fileio::load(&file, "windows-1252", |_| {}).is_err());
            }
            _ => std::fs::remove_file(&file).unwrap(),
        }
        let mut restored = Workspace::new(Text::new(), None, None, false).unwrap();
        restored
            .restore_window(&session.windows[0], &session.documents)
            .unwrap();
        assert!(restored.tabs[1].lazy.is_some());
        for index in [1, 2, 1] {
            restored.prepare_sheet(index).unwrap();
            assert_eq!(restored.tabs[index].editor.text, expected, "{failure}");
            assert!(!restored.tabs[index].editor.read_only);
            assert!(restored.tabs[index].dirty());
        }
        let (window, documents) = restored.snapshot().unwrap();
        let again = dir.path().join(format!("{failure}.sublime-session"));
        vel_session::save(
            &again,
            &vel_session::Session {
                windows: vec![window],
                documents,
                ..Default::default()
            },
        )
        .unwrap();
        let reloaded = vel_session::load(&again);
        assert!(reloaded.messages.is_empty(), "{:?}", reloaded.messages);
        let document = reloaded
            .session
            .documents
            .iter()
            .find(|d| d.path.is_some())
            .unwrap();
        assert_eq!(
            document.history.as_ref().unwrap().current_text().unwrap(),
            expected
        );
        if file.exists() {
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
    }
}
