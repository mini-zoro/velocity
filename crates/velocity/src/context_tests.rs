use super::*;
use crate::{cli::FileLocation, editor::Editor, workspace::Workspace};
use std::time::{Duration, Instant};
use vel_buffer::Selection;
use vel_config::resources::Resources;
use vel_text::Text;

#[derive(Debug)]
struct TestPlatform {
    trash: PathBuf,
    trashed: Vec<PathBuf>,
    fail: Vec<PathBuf>,
    clipboard: Option<String>,
    revealed: Vec<PathBuf>,
}
impl TestPlatform {
    fn new(root: &Path) -> Self {
        let trash = root.join("test-trash");
        fs::create_dir(&trash).unwrap();
        Self {
            trash,
            trashed: Vec::new(),
            fail: Vec::new(),
            clipboard: None,
            revealed: Vec::new(),
        }
    }
}
impl SidebarPlatform for TestPlatform {
    fn trash(&mut self, path: &Path) -> Result<(), String> {
        if self.fail.contains(&path.to_owned()) {
            return Err("injected Trash failure".into());
        }
        let destination = self.trash.join(format!(
            "{}-{}",
            self.trashed.len(),
            path.file_name().unwrap().to_string_lossy()
        ));
        fs::rename(path, destination).map_err(|e| e.to_string())?;
        self.trashed.push(path.to_owned());
        Ok(())
    }
    fn copy_text(&mut self, text: &str) -> Result<(), String> {
        self.clipboard = Some(text.into());
        Ok(())
    }
    fn reveal(&mut self, path: &Path) -> Result<(), String> {
        self.revealed.push(path.to_owned());
        Ok(())
    }
}
fn execute(name: &str, args: &Value, root: &Path, platform: &mut TestPlatform) -> SidebarEffect {
    sidebar_command(name, args, &[root.to_owned()], platform)
        .unwrap()
        .unwrap()
}
fn callback(effect: SidebarEffect, text: &str) -> Invocation {
    let SidebarEffect::Input(prompt) = effect else {
        panic!("expected input panel");
    };
    assert_eq!(prompt.command, "show_input_panel");
    let mut callback: Invocation = serde_json::from_value(prompt.args["on_done"].clone()).unwrap();
    callback.args["text"] = json!(text);
    callback
}
fn workspace_file(path: &Path) -> Workspace {
    let mut workspace = Workspace::new(Text::new(), None, None, false).unwrap();
    workspace
        .open(FileLocation {
            path: path.to_owned(),
            line: 1,
            column: 1,
        })
        .unwrap();
    workspace
}

#[test]
fn original_resources_parse_dynamic_rows_keep_order_and_route_identical_context() {
    let mut resources = Resources::default();
    for (name, source) in [
        (
            ContextMenuKind::Editor,
            include_str!("../resources/Packages/Default/Context.sublime-menu"),
        ),
        (
            ContextMenuKind::Tab,
            include_str!("../resources/Packages/Default/Tab Context.sublime-menu"),
        ),
        (
            ContextMenuKind::SideBar,
            include_str!("../resources/Packages/Default/Side Bar.sublime-menu"),
        ),
    ] {
        resources.insert(
            &format!("Packages/Default/{}", name.resource_name()),
            source.as_bytes().to_vec(),
            PathBuf::from("<test>"),
        );
        assert!(
            !ui::load_menu(&resources, name.resource_name())
                .unwrap()
                .is_empty()
        );
    }
    resources.insert(
        "Packages/User/Context.sublime-menu",
        br#"[{"id":"velocity_copy","caption":"My Copy","command":"copy","args":{"custom":true}}]"#
            .to_vec(),
        PathBuf::from("<user>"),
    );
    let definitions = ui::load_menu(&resources, ContextMenuKind::Editor.resource_name()).unwrap();
    let dynamic = vec![PaletteItem {
        caption: "Suggestion".into(),
        invocation: Invocation {
            command: "replace_word".into(),
            args: json!({"word":"fixed"}),
        },
    }];
    let expanded = with_dynamic(&definitions, &dynamic);
    let context = MenuContext {
        paths: vec![PathBuf::from("/tmp/example.txt")],
        group: 2,
        index: 3,
        event: Point { x: 20.0, y: 40.0 },
    };
    let rows = resolve_menu(&expanded, &context, |invocation| {
        assert_eq!(invocation.args["paths"], json!(["/tmp/example.txt"]));
        assert_eq!(invocation.args["group"], 2);
        assert_eq!(invocation.args["index"], 3);
        assert_eq!(invocation.args["event"], json!({"x":20.0,"y":40.0}));
        CommandState {
            checked: invocation.command == "replace_word",
            ..Default::default()
        }
    });
    assert_eq!(rows[1].caption, "My Copy");
    assert_eq!(rows[1].invocation.as_ref().unwrap().args["custom"], true);
    let suggestion = rows.iter().position(|r| r.caption == "Suggestion").unwrap();
    assert!(
        suggestion
            > rows
                .iter()
                .position(|r| r
                    .invocation
                    .as_ref()
                    .is_some_and(|i| i.command == "select_all"))
                .unwrap()
    );
    assert!(
        suggestion
            < rows
                .iter()
                .position(|r| r
                    .invocation
                    .as_ref()
                    .is_some_and(|i| i.command == "copy_file_path"))
                .unwrap()
    );
    assert!(rows[suggestion].checked);
    assert_eq!(
        rows[suggestion].invocation.as_ref().unwrap().args["word"],
        "fixed"
    );
    assert!(definitions.iter().any(|i| i.id == "velocity_dynamic"));
    let mut enriched = context.arguments();
    enriched["_context_sheet"] = json!(77);
    enriched["_context_menu"] = json!("editor");
    let rows = resolve_menu(&expanded, &enriched, |_| CommandState::default());
    assert!(
        rows.iter()
            .filter_map(|r| r.invocation.as_ref())
            .all(|i| i.args["_context_sheet"] == 77 && i.args["_context_menu"] == "editor")
    );
}

#[test]
fn dynamic_rows_append_to_user_replacements_and_empty_markers_disappear() {
    let definitions: Vec<MenuItem> = ui::parse(r#"[{"caption":"User","command":"user"}]"#).unwrap();
    let dynamic = [PaletteItem {
        caption: "Git History".into(),
        invocation: Invocation {
            command: "git_file_history".into(),
            args: json!({}),
        },
    }];
    let rows = resolve_menu(
        &with_dynamic(&definitions, &dynamic),
        &MenuContext::default(),
        |_| CommandState::default(),
    );
    assert_eq!(
        rows.iter().map(|r| r.caption.as_str()).collect::<Vec<_>>(),
        ["User", "-", "Git History"]
    );
    let definitions: Vec<MenuItem> =
        ui::parse(r#"[{"id":"velocity_dynamic"},{"caption":"-"}]"#).unwrap();
    assert!(
        resolve_menu(
            &with_dynamic(&definitions, &[]),
            &MenuContext::default(),
            |_| CommandState::default()
        )
        .is_empty()
    );
}

#[test]
fn right_click_preserves_reversed_and_multiple_selections_including_edges() {
    let mut editor = Editor::new(Text::from("zero one two three"));
    editor.set_selection(Selection::from_regions([
        Region::new(8, 5),
        Region::new(13, 18),
    ]));
    let before = editor.regions();
    for byte in [5, 6, 8, 13, 16, 18] {
        assert!(right_click_preserves_selection(&editor.regions(), byte));
        assert_eq!(editor.regions(), before);
    }
    assert!(!right_click_preserves_selection(&editor.regions(), 10));
    if !right_click_preserves_selection(&editor.regions(), 10) {
        editor.set_caret(10, false);
    }
    assert_eq!(editor.regions(), [Region::caret(10)]);
    assert!(right_click_preserves_selection(&editor.regions(), 10));
    assert!(!right_click_preserves_selection(&[], 0));
}

#[test]
fn tab_close_variants_use_clicked_visual_order_and_keep_stable_sheet_ids() {
    let tabs = [
        TabSnapshot {
            sheet: 90,
            dirty: false,
        },
        TabSnapshot {
            sheet: 7,
            dirty: true,
        },
        TabSnapshot {
            sheet: 43,
            dirty: false,
        },
        TabSnapshot {
            sheet: 2,
            dirty: true,
        },
    ];
    assert_eq!(tab_close_targets(&tabs, 1, TabCloseKind::Current), [7]);
    assert_eq!(
        tab_close_targets(&tabs, 1, TabCloseKind::Others),
        [2, 43, 90]
    );
    assert_eq!(tab_close_targets(&tabs, 1, TabCloseKind::ToRight), [2, 43]);
    assert_eq!(
        tab_close_targets(&tabs, 1, TabCloseKind::Unmodified),
        [43, 90]
    );
    assert!(tab_close_targets(&tabs, 3, TabCloseKind::ToRight).is_empty());
    for kind in [
        TabCloseKind::Current,
        TabCloseKind::Others,
        TabCloseKind::ToRight,
        TabCloseKind::Unmodified,
    ] {
        assert!(tab_close_targets(&tabs, tabs.len(), kind).is_empty());
        assert!(tab_close_targets(&[], 0, kind).is_empty());
    }
    assert_eq!(
        TabCloseKind::from_command("close_file"),
        Some(TabCloseKind::Current)
    );
    assert_eq!(
        TabCloseKind::from_command("close_other_tabs"),
        Some(TabCloseKind::Others)
    );
    assert_eq!(
        TabCloseKind::from_command("close_tabs_to_right"),
        Some(TabCloseKind::ToRight)
    );
    assert_eq!(
        TabCloseKind::from_command("close_unmodified_tabs"),
        Some(TabCloseKind::Unmodified)
    );
    assert_eq!(TabCloseKind::from_command("unrelated"), None);
}

#[test]
fn name_prompts_roundtrip_context_and_create_files_and_folders_without_clobbering() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let existing = root.join("existing.txt");
    fs::write(&existing, "retained").unwrap();
    let mut platform = TestPlatform::new(root);
    let args = json!({"paths":[existing],"group":2,"index":1,"event":{"x":1,"y":2}});
    let prompt = execute("side_bar_new_file", &args, root, &mut platform);
    assert_eq!(fs::read_to_string(&existing).unwrap(), "retained");
    let callback = callback(prompt, "new 文.txt");
    for key in ["paths", "group", "index", "event"] {
        assert_eq!(callback.args[key], args[key]);
    }
    let created = root.join("new 文.txt");
    assert_eq!(
        execute(&callback.command, &callback.args, root, &mut platform),
        SidebarEffect::Created {
            path: created.clone(),
            directory: false
        }
    );
    assert_eq!(fs::read_to_string(&created).unwrap(), "");
    assert!(
        sidebar_command(
            "side_bar_new_file",
            &callback.args,
            &[root.to_owned()],
            &mut platform
        )
        .is_err()
    );
    let folder_args = json!({"paths":[root],"text":"nested"});
    let nested = root.join("nested");
    assert_eq!(
        execute("side_bar_new_folder", &folder_args, root, &mut platform),
        SidebarEffect::Created {
            path: nested.clone(),
            directory: true
        }
    );
    assert!(nested.is_dir());
    for name in [
        "",
        ".",
        "..",
        "../escape",
        "sub/file",
        "sub\\file",
        "/absolute",
        "bad\0name",
    ] {
        for command in [
            "side_bar_new_file",
            "side_bar_new_folder",
            "side_bar_rename",
        ] {
            assert!(
                sidebar_command(
                    command,
                    &json!({"paths":[existing],"text":name}),
                    &[root.to_owned()],
                    &mut platform
                )
                .is_err(),
                "{command}: {name:?}"
            );
        }
    }
    assert_eq!(fs::read_to_string(&existing).unwrap(), "retained");
    assert!(platform.trashed.is_empty());
}

#[test]
fn rename_prompts_reject_existing_targets_and_retarget_prefixes_componentwise() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let old = root.join("old.txt");
    let occupied = root.join("occupied.txt");
    fs::write(&old, "source").unwrap();
    fs::write(&occupied, "destination").unwrap();
    let mut platform = TestPlatform::new(root);
    let prompt = execute(
        "side_bar_rename",
        &json!({"paths":[old]}),
        root,
        &mut platform,
    );
    let SidebarEffect::Input(ref invocation) = prompt else {
        panic!("input");
    };
    assert_eq!(invocation.args["initial_text"], "old.txt");
    let callback = callback(prompt, "occupied.txt");
    assert!(
        sidebar_command(
            &callback.command,
            &callback.args,
            &[root.to_owned()],
            &mut platform
        )
        .is_err()
    );
    assert_eq!(fs::read_to_string(&occupied).unwrap(), "destination");
    let new = root.join("renamed.rs");
    assert_eq!(
        execute(
            "side_bar_rename",
            &json!({"paths":[old],"text":"renamed.rs"}),
            root,
            &mut platform
        ),
        SidebarEffect::Renamed {
            from: old.clone(),
            to: new.clone()
        }
    );
    assert!(!old.exists());
    assert_eq!(fs::read_to_string(&new).unwrap(), "source");
    let mapped = retarget_path(&old, &old, &new).unwrap();
    assert_eq!(
        mapped.as_os_str(),
        new.as_os_str(),
        "a file retarget must not append a slash"
    );
    assert_eq!(
        retarget_path(
            Path::new("/project/src/a.rs"),
            Path::new("/project/src"),
            Path::new("/project/lib")
        ),
        Some(PathBuf::from("/project/lib/a.rs"))
    );
    assert!(
        retarget_path(
            Path::new("/project/src-old/a.rs"),
            Path::new("/project/src"),
            Path::new("/project/lib")
        )
        .is_none()
    );
}

#[test]
fn trash_requires_boolean_confirmation_preserves_files_and_reports_partial_failures() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let a = root.join("a.txt");
    let b = root.join("b.txt");
    fs::write(&a, "keep in trash").unwrap();
    fs::write(&b, "failed entry").unwrap();
    let mut platform = TestPlatform::new(root);
    platform.fail.push(b.clone());
    let args = json!({"paths":[a,a,b],"confirmed":"true","event":{"x":3,"y":4}});
    let effect = execute("side_bar_delete", &args, root, &mut platform);
    assert!(a.exists() && b.exists());
    assert!(platform.trashed.is_empty());
    let SidebarEffect::ConfirmDelete { paths, on_confirm } = effect else {
        panic!("confirmation");
    };
    assert_eq!(paths, [a.clone(), b.clone()]);
    assert_eq!(on_confirm.args["event"], args["event"]);
    let SidebarEffect::Deleted { paths, failures } =
        execute(&on_confirm.command, &on_confirm.args, root, &mut platform)
    else {
        panic!("deleted effect");
    };
    assert_eq!(paths.as_slice(), std::slice::from_ref(&a));
    assert_eq!(failures.len(), 1);
    assert!(failures[0].contains(&b.display().to_string()));
    assert_eq!(platform.trashed, [a]);
    assert!(!paths[0].exists() && b.exists());
    let trash_file = fs::read_dir(&platform.trash)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    assert_eq!(fs::read_to_string(trash_file).unwrap(), "keep in trash");
}

#[test]
fn trash_covers_descendants_once_and_prevalidates_all_entries() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let folder = root.join("folder");
    fs::create_dir(&folder).unwrap();
    let file = folder.join("file.txt");
    fs::write(&file, "child").unwrap();
    let mut platform = TestPlatform::new(root);
    assert!(
        sidebar_command(
            "side_bar_delete",
            &json!({"paths":[folder,root.join("missing")],"confirmed":true}),
            &[root.to_owned()],
            &mut platform
        )
        .is_err()
    );
    assert!(platform.trashed.is_empty() && file.exists());
    let effect = execute(
        "side_bar_delete",
        &json!({"paths":[file,folder],"confirmed":true}),
        root,
        &mut platform,
    );
    assert_eq!(
        effect,
        SidebarEffect::Deleted {
            paths: vec![folder.clone()],
            failures: vec![]
        }
    );
    assert_eq!(platform.trashed, [folder]);
    assert!(path_was_deleted(&file, &platform.trashed));
    assert!(!path_was_deleted(
        &root.join("folder-copy/file.txt"),
        &platform.trashed
    ));
    assert!(
        sidebar_command(
            "side_bar_delete",
            &json!({"paths":["/"],"confirmed":true}),
            &[root.to_owned()],
            &mut platform
        )
        .is_err()
    );
}

#[test]
fn clipboard_reveal_find_and_root_removal_use_the_injected_services() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let nested = root.join("nested");
    fs::create_dir(&nested).unwrap();
    let file = nested.join("a.txt");
    fs::write(&file, "text").unwrap();
    let mut platform = TestPlatform::new(root);
    assert_eq!(
        execute(
            "copy_file_path",
            &json!({"paths":[file,root]}),
            root,
            &mut platform
        ),
        SidebarEffect::Done
    );
    assert_eq!(
        platform.clipboard.as_deref(),
        Some(format!("{}\n{}", file.display(), root.display()).as_str())
    );
    sidebar_command(
        "copy_relative_path",
        &json!({"paths":[file,root]}),
        &[root.to_owned(), nested.clone()],
        &mut platform,
    )
    .unwrap();
    assert_eq!(platform.clipboard.as_deref(), Some("a.txt\n."));
    assert!(
        sidebar_command(
            "copy_relative_path",
            &json!({"paths":[root]}),
            std::slice::from_ref(&nested),
            &mut platform
        )
        .is_err()
    );
    assert_eq!(platform.clipboard.as_deref(), Some("a.txt\n."));
    execute(
        "reveal_in_finder",
        &json!({"paths":[file]}),
        root,
        &mut platform,
    );
    assert_eq!(platform.revealed.as_slice(), std::slice::from_ref(&file));
    assert_eq!(
        execute(
            "side_bar_find_in_folder",
            &json!({"paths":[file]}),
            root,
            &mut platform
        ),
        SidebarEffect::FindInFolder(nested.clone())
    );
    assert_eq!(
        execute(
            "side_bar_find_in_folder",
            &json!({"paths":[nested]}),
            root,
            &mut platform
        ),
        SidebarEffect::FindInFolder(nested.clone())
    );
    assert_eq!(
        execute(
            "side_bar_remove_folder",
            &json!({"paths":[root]}),
            root,
            &mut platform
        ),
        SidebarEffect::RemoveFolder(root.to_owned())
    );
    assert!(
        sidebar_command(
            "side_bar_remove_folder",
            &json!({"paths":[nested]}),
            &[root.to_owned()],
            &mut platform
        )
        .is_err()
    );
    assert!(root.exists() && file.exists());
    assert_eq!(
        execute(
            "reveal_in_side_bar",
            &json!({"paths":[file]}),
            root,
            &mut platform
        ),
        SidebarEffect::RevealInSideBar(file.clone())
    );
    assert_eq!(
        execute(
            "side_bar_open",
            &json!({"paths":[file]}),
            root,
            &mut platform
        ),
        SidebarEffect::Open(vec![file])
    );
    assert!(
        sidebar_command("unrelated", &json!(null), &[], &mut platform)
            .unwrap()
            .is_none()
    );
}

#[test]
fn sidebar_state_handles_missing_multiselect_and_project_roots() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().to_owned();
    let file = root.join("entry");
    fs::write(&file, "text").unwrap();
    let selected = json!({"paths":[file]});
    let roots = [root.clone()];
    let remove = sidebar_command_state("side_bar_remove_folder", &selected, &roots).unwrap();
    assert!(!remove.visible && !remove.enabled);
    assert!(
        sidebar_command_state("side_bar_remove_folder", &json!({"paths":[root]}), &roots)
            .unwrap()
            .enabled
    );
    assert!(
        sidebar_command_state("side_bar_rename", &selected, &roots)
            .unwrap()
            .enabled
    );
    assert!(
        !sidebar_command_state("side_bar_rename", &json!({"paths":[file,root]}), &roots)
            .unwrap()
            .enabled
    );
    assert!(
        !sidebar_command_state(
            "side_bar_open",
            &json!({"paths":[root.join("missing")]}),
            &roots
        )
        .unwrap()
        .enabled
    );
    for command in COMMANDS.iter().filter(|name| !name.starts_with("close_")) {
        assert!(
            !sidebar_command_state(command, &json!({"paths":[]}), &roots)
                .unwrap()
                .enabled
        );
    }
    assert!(sidebar_command_state("custom_package_command", &selected, &roots).is_none());
}

#[test]
fn rename_and_trash_preserve_dirty_shared_tabs_undo_and_unrelated_buffers() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let folder = root.join("src");
    fs::create_dir(&folder).unwrap();
    let old = folder.join("file.txt");
    fs::write(&old, "saved").unwrap();
    let mut workspace = workspace_file(&old);
    workspace
        .editor_mut()
        .run("insert", &json!({"characters":"unsaved "}))
        .unwrap();
    workspace.clone_file().unwrap();
    workspace.sync_shared();
    let document = workspace.tabs[0].document;
    let sheets: Vec<_> = workspace.tabs.iter().map(|t| t.sheet).collect();
    let selected = workspace.editor().regions();
    let mut platform = TestPlatform::new(&root);
    prepare_file_operation(&mut workspace, std::slice::from_ref(&folder)).unwrap();
    let renamed = root.join("lib");
    let SidebarEffect::Renamed { from, to } = execute(
        "side_bar_rename",
        &json!({"paths":[folder],"text":"lib"}),
        &root,
        &mut platform,
    ) else {
        panic!("renamed");
    };
    apply_rename(&mut workspace, &from, &to).unwrap();
    let new_file = renamed.join("file.txt");
    for tab in &workspace.tabs {
        assert_eq!(tab.path.as_ref(), Some(&new_file));
        assert_eq!(tab.editor.snippet_path.as_ref(), Some(&new_file));
        assert_eq!(tab.document, document);
        assert!(!tab.deleted);
        assert!(tab.dirty());
    }
    assert_eq!(workspace.editor().regions(), selected);
    assert_eq!(workspace.editor().text.to_string(), "unsaved saved");
    prepare_file_operation(&mut workspace, std::slice::from_ref(&renamed)).unwrap();
    let SidebarEffect::Deleted { paths, failures } = execute(
        "side_bar_delete",
        &json!({"paths":[renamed],"confirmed":true}),
        &root,
        &mut platform,
    ) else {
        panic!("deleted");
    };
    assert!(failures.is_empty());
    apply_delete(&mut workspace, &paths);
    assert_eq!(
        workspace.tabs.iter().map(|t| t.sheet).collect::<Vec<_>>(),
        sheets
    );
    assert!(
        workspace
            .tabs
            .iter()
            .all(|t| t.deleted && t.path.as_ref() == Some(&new_file))
    );
    assert_eq!(workspace.editor().text.to_string(), "unsaved saved");
    workspace.editor_mut().run("undo", &json!({})).unwrap();
    assert_eq!(workspace.editor().text.to_string(), "saved");
    assert!(
        workspace
            .tabs
            .iter()
            .all(|t| t.label().contains("[deleted]"))
    );
}

#[test]
fn rename_rearms_file_watcher_and_delete_leaves_other_tabs_untouched() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let old = root.join("old.txt");
    let other = root.join("other.txt");
    fs::write(&old, "saved").unwrap();
    fs::write(&other, "other buffer").unwrap();
    let mut workspace = workspace_file(&old);
    workspace
        .open(FileLocation {
            path: other.clone(),
            line: 1,
            column: 1,
        })
        .unwrap();
    let mut watcher = vel_fileio::Watcher::new().unwrap();
    watcher.watch(&old).unwrap();
    workspace.watcher = Some(watcher);
    let new = root.join("new.txt");
    fs::rename(&old, &new).unwrap();
    apply_rename(&mut workspace, &old, &new).unwrap();
    fs::write(&new, "external content now longer").unwrap();
    let until = Instant::now() + Duration::from_secs(5);
    while workspace.tabs[0].editor.text != "external content now longer" {
        workspace.poll();
        assert!(
            Instant::now() < until,
            "watcher did not follow renamed file: {:?}",
            workspace.messages
        );
        std::thread::park_timeout(Duration::from_millis(2));
    }
    apply_delete(&mut workspace, &[new]);
    assert!(workspace.tabs[0].deleted);
    assert!(!workspace.tabs[1].deleted);
    assert_eq!(workspace.tabs[1].path.as_ref(), Some(&other));
    assert_eq!(workspace.tabs[1].editor.text.to_string(), "other buffer");
}

#[cfg(unix)]
#[test]
fn operations_rename_and_trash_symlink_entries_without_following_their_targets() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let actual = root.join("actual.txt");
    let alias = root.join("alias.txt");
    fs::write(&actual, "target must survive").unwrap();
    std::os::unix::fs::symlink(&actual, &alias).unwrap();
    let mut platform = TestPlatform::new(root);
    let renamed = root.join("renamed-link");
    execute(
        "side_bar_rename",
        &json!({"paths":[alias],"text":"renamed-link"}),
        root,
        &mut platform,
    );
    assert!(
        fs::symlink_metadata(&renamed)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    execute(
        "side_bar_delete",
        &json!({"paths":[renamed],"confirmed":true}),
        root,
        &mut platform,
    );
    assert_eq!(fs::read_to_string(&actual).unwrap(), "target must survive");
    assert_eq!(platform.trashed, [renamed]);
    let broken = root.join("broken");
    std::os::unix::fs::symlink(root.join("not-present"), &broken).unwrap();
    execute(
        "side_bar_delete",
        &json!({"paths":[broken],"confirmed":true}),
        root,
        &mut platform,
    );
    assert_eq!(platform.trashed.len(), 2);
}

#[test]
fn preflight_materializes_hidden_lazy_files_before_they_are_trashed() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let hidden = root.join("hidden.txt");
    let focused = root.join("focused.txt");
    fs::write(&hidden, "retained hidden contents").unwrap();
    fs::write(&focused, "focused").unwrap();
    let mut initial = workspace_file(&hidden);
    initial
        .open(FileLocation {
            path: focused.clone(),
            line: 1,
            column: 1,
        })
        .unwrap();
    let (window, documents) = initial.snapshot().unwrap();
    let mut restored = Workspace::new(Text::new(), None, None, false).unwrap();
    restored.restore_window(&window, &documents).unwrap();
    let hidden_index = restored
        .tabs
        .iter()
        .position(|t| t.path.as_ref() == Some(&hidden))
        .unwrap();
    assert!(restored.tabs[hidden_index].lazy.is_some());
    let active = restored.tabs[restored.active].sheet;
    prepare_file_operation(&mut restored, std::slice::from_ref(&hidden)).unwrap();
    assert!(restored.tabs[hidden_index].lazy.is_none());
    assert_eq!(restored.tabs[restored.active].sheet, active);
    assert_eq!(
        restored.tabs[hidden_index].editor.text.to_string(),
        "retained hidden contents"
    );
    let mut platform = TestPlatform::new(&root);
    platform.trash(&hidden).unwrap();
    apply_delete(&mut restored, std::slice::from_ref(&hidden));
    assert!(restored.tabs[hidden_index].deleted);
    assert!(!hidden.exists());
    restored.select(hidden_index);
    assert_eq!(
        restored.editor().text.to_string(),
        "retained hidden contents"
    );
}

#[test]
fn preflight_refuses_pending_background_loads_without_mutating_the_file() {
    let directory = tempfile::tempdir().unwrap();
    let file = directory
        .path()
        .canonicalize()
        .unwrap()
        .join("background.txt");
    fs::write(&file, "background contents").unwrap();
    let mut workspace = Workspace::new(Text::new(), None, None, false).unwrap();
    workspace.settings.set(
        vel_config::settings::Layer::User,
        "large_file.load_bytes",
        json!(1),
    );
    workspace
        .open(FileLocation {
            path: file.clone(),
            line: 1,
            column: 1,
        })
        .unwrap();
    assert!(workspace.tabs[workspace.active].loading.is_some());
    let error = prepare_file_operation(&mut workspace, std::slice::from_ref(&file)).unwrap_err();
    assert!(error.contains("finish loading"));
    assert_eq!(fs::read_to_string(&file).unwrap(), "background contents");
}

#[test]
fn rename_keeps_deleted_markers_and_disk_state_and_noop_rename_changes_nothing() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let folder = root.join("src");
    fs::create_dir(&folder).unwrap();
    let gone = folder.join("gone.txt");
    let kept = folder.join("kept.txt");
    fs::write(&gone, "only copy").unwrap();
    fs::write(&kept, "kept").unwrap();
    let mut workspace = workspace_file(&gone);
    workspace
        .open(FileLocation {
            path: kept.clone(),
            line: 1,
            column: 1,
        })
        .unwrap();
    let mut platform = TestPlatform::new(&root);
    let SidebarEffect::Deleted { paths, .. } = execute(
        "side_bar_delete",
        &json!({"paths":[gone],"confirmed":true}),
        &root,
        &mut platform,
    ) else {
        panic!("deleted");
    };
    apply_delete(&mut workspace, &paths);
    assert!(workspace.tabs[0].deleted && workspace.tabs[0].dirty());
    // A pending "changed on disk" state of another buffer must survive a rename.
    let stamp = vel_session::DiskStamp {
        bytes: 1,
        modified_ns: 1,
    };
    workspace.tabs[1].disk_saved = Some(stamp.clone());
    // Pressing Enter without changing the name is a no-op.
    let SidebarEffect::Renamed { from, to } = execute(
        "side_bar_rename",
        &json!({"paths":[folder],"text":"src"}),
        &root,
        &mut platform,
    ) else {
        panic!("renamed");
    };
    apply_rename(&mut workspace, &from, &to).unwrap();
    assert!(workspace.tabs[0].deleted && workspace.tabs[0].dirty());
    let SidebarEffect::Renamed { from, to } = execute(
        "side_bar_rename",
        &json!({"paths":[folder],"text":"lib"}),
        &root,
        &mut platform,
    ) else {
        panic!("renamed");
    };
    apply_rename(&mut workspace, &from, &to).unwrap();
    let tab = &workspace.tabs[0];
    assert_eq!(
        tab.path.as_deref(),
        Some(root.join("lib/gone.txt").as_path())
    );
    assert!(
        tab.deleted && tab.dirty(),
        "the only copy must still prompt on close"
    );
    assert!(tab.label().contains("[deleted]"));
    let tab = &workspace.tabs[1];
    assert_eq!(
        tab.path.as_deref(),
        Some(root.join("lib/kept.txt").as_path())
    );
    assert!(!tab.deleted);
    assert_eq!(tab.disk_saved, Some(stamp));
}

#[test]
fn rename_refuses_existing_names_and_allows_case_only_changes() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let file = root.join("note.txt");
    let other = root.join("other.txt");
    fs::write(&file, "note").unwrap();
    fs::write(&other, "other").unwrap();
    let mut platform = TestPlatform::new(&root);
    let error = sidebar_command(
        "side_bar_rename",
        &json!({"paths":[file],"text":"other.txt"}),
        std::slice::from_ref(&root),
        &mut platform,
    )
    .unwrap_err();
    assert!(error.contains("already exists"), "{error}");
    assert_eq!(fs::read_to_string(&other).unwrap(), "other");
    let SidebarEffect::Renamed { to, .. } = execute(
        "side_bar_rename",
        &json!({"paths":[file],"text":"Note.txt"}),
        &root,
        &mut platform,
    ) else {
        panic!("renamed");
    };
    assert_eq!(to, root.join("Note.txt"));
    let names: Vec<_> = fs::read_dir(&root)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(names.contains(&"Note.txt".into()) && !names.contains(&"note.txt".into()));
}

#[test]
fn renaming_a_relative_project_root_updates_the_project_file() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    fs::create_dir(root.join("code")).unwrap();
    let project = root.join("p.sublime-project");
    fs::write(&project, r#"{"folders":[{"path":"code"}],"custom":1}"#).unwrap();
    let mut workspace = Workspace::new(Text::new(), None, Some(project.clone()), false).unwrap();
    let mut platform = TestPlatform::new(&root);
    let SidebarEffect::Renamed { from, to } = execute(
        "side_bar_rename",
        &json!({"paths":[root.join("code")],"text":"source"}),
        &root,
        &mut platform,
    ) else {
        panic!("renamed");
    };
    apply_rename(&mut workspace, &from, &to).unwrap();
    let saved = vel_session::project::Project::load(&project).unwrap();
    assert_eq!(saved.folders.len(), 1);
    assert_eq!(saved.folders[0].path, Path::new("source"));
    assert_eq!(saved.extra["custom"], 1);
    workspace.wait_resources();
    let roots: Vec<_> = workspace.folders.iter().map(|f| f.path.clone()).collect();
    assert_eq!(
        roots,
        [root.join("source")],
        "no ghost of the old root after a reload"
    );
}

#[cfg(unix)]
#[test]
fn entries_below_a_symlinked_folder_match_their_canonical_tabs() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let real = root.join("real");
    fs::create_dir(&real).unwrap();
    std::os::unix::fs::symlink(&real, root.join("alias")).unwrap();
    let file = real.join("a.txt");
    fs::write(&file, "text").unwrap();
    let mut workspace = workspace_file(&file);
    assert_eq!(workspace.tabs[0].path.as_deref(), Some(file.as_path()));
    // The side bar spells the entry through the symlinked folder.
    let spelled = root.join("alias").join("a.txt");
    let renamed = root.join("alias").join("b.txt");
    fs::rename(&spelled, &renamed).unwrap();
    apply_rename(&mut workspace, &spelled, &renamed).unwrap();
    assert_eq!(
        workspace.tabs[0].path.as_deref(),
        Some(real.join("b.txt").as_path())
    );
    apply_delete(&mut workspace, std::slice::from_ref(&renamed));
    assert!(workspace.tabs[0].deleted);
    // Trashing the symlink entry itself does not delete the files it points to.
    let mut other = workspace_file(&real.join("b.txt"));
    apply_delete(&mut other, &[root.join("alias")]);
    assert!(!other.tabs[0].deleted);
}
