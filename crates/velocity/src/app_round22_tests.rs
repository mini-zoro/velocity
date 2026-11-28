use super::*;
use crate::cli::FileLocation;
use std::{fs, path::Path};
use vel_config::keymap::Key;
use vel_text::Text;

fn profile(root: &Path, preferences: &str) {
    let user = root.join("Packages/User");
    fs::create_dir_all(&user).unwrap();
    fs::write(user.join("Preferences.sublime-settings"), preferences).unwrap();
}

fn app(root: &Path) -> App {
    crate::test_login::install();
    let options = Options::default();
    App::new(
        options.clone(),
        Clock::new(Instant::now(), std::time::SystemTime::now()),
        Reporter::for_options(&options, Instant::now()).unwrap(),
        Workspace::new(Text::new(), Some(root.to_path_buf()), None, false).unwrap(),
        Arc::new(AtomicBool::new(false)),
        std::rc::Rc::new(std::cell::RefCell::new(crate::session::Sessions::new(
            Some(root),
        ))),
    )
}

fn location(path: &Path) -> FileLocation {
    FileLocation {
        path: path.into(),
        line: 1,
        column: 1,
    }
}

fn key(app: &mut App, chord: &str) -> bool {
    app.dispatch_key_binding(
        None,
        &Key::parse(chord).unwrap(),
        Instant::now(),
        app.viewport(),
    )
}

#[cfg(unix)]
#[test]
fn save_dispatch_default_and_adaptive_mode_keep_the_inode_and_finish_clean() {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let root = tempfile::tempdir().unwrap();
    let files = root.path().join("files");
    fs::create_dir(&files).unwrap();
    let file = files.join("test.txt");
    for atomic in [false, true] {
        profile(
            root.path(),
            if atomic {
                r#"{"atomic_save":true}"#
            } else {
                "{}"
            },
        );
        fs::write(&file, "original").unwrap();
        let inode = file.metadata().unwrap().ino();
        let mut a = app(root.path());
        a.workspace.open(location(&file)).unwrap();
        assert_eq!(a.workspace.editor().settings.editor().atomic_save, atomic);
        if atomic {
            fs::set_permissions(&files, fs::Permissions::from_mode(0o555)).unwrap();
        }
        a.run_named(None, "insert", &json!({"characters":"new "}));
        assert!(key(&mut a, "super+s"));
        fs::set_permissions(&files, fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(file.metadata().unwrap().ino(), inode);
        assert_eq!(fs::read_to_string(&file).unwrap(), "new original");
        assert!(
            !a.workspace.tabs[a.workspace.active].dirty(),
            "{}",
            a.panels.message
        );

        fs::set_permissions(&file, fs::Permissions::from_mode(0o400)).unwrap();
        a.run_named(None, "insert", &json!({"characters":"unsaved "}));
        assert!(key(&mut a, "super+s"));
        assert!(a.workspace.tabs[a.workspace.active].dirty());
        let detail = &a
            .data_loss_dialogs
            .last()
            .expect("failed save dialog")
            .detail;
        assert!(detail.contains("Permission denied"), "{detail}");
        assert_eq!(fs::read_to_string(&file).unwrap(), "new original");
        fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();
    }
}

#[cfg(unix)]
#[test]
fn save_shortcut_recovers_its_interrupted_revision_before_disk_conflict_check() {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt};
    let root = tempfile::tempdir().unwrap();
    profile(root.path(), "{}");
    let target = root.path().join("doc.txt");
    let old = "old revision text";
    let pending = "pending revision";
    fs::write(&target, old).unwrap();
    let mut a = app(root.path());
    a.open_received_paths(std::slice::from_ref(&target))
        .unwrap();
    a.run_named(None, "insert", &json!({"characters":"live "}));

    // Write the exact on-disk fixture left by an interrupted forward journal,
    // after the app loaded the original bytes and before the retry.
    let recovery = root.path().join("Recovery");
    fs::DirBuilder::new().mode(0o700).create(&recovery).unwrap();
    let stage = root.path().join(".velocity-save-PartialFixture");
    fs::write(&stage, pending).unwrap();
    let absolute = |path: &Path| {
        path.canonicalize()
            .unwrap()
            .as_os_str()
            .as_encoded_bytes()
            .to_vec()
    };
    let entry = |path: &Path| {
        let meta = path.metadata().unwrap();
        json!({"device":meta.dev(), "inode":meta.ino()})
    };
    let metadata = target.metadata().unwrap();
    let record = recovery.join(format!(
        "{:016x}-{:016x}.save-journal",
        metadata.dev(),
        metadata.ino()
    ));
    fs::write(&record, serde_json::to_vec(&json!({
        "version":2, "target":absolute(&target), "target_entry":entry(&target),
        "stage":absolute(&stage), "stage_entry":entry(&stage), "stage_len":pending.len(),
        "content":vel_fileio::content_identity(&stage).unwrap().unwrap(),
        "mode":metadata.mode() & 0o7777, "original_len":old.len(), "original":{"Inline":old.as_bytes()}
    })).unwrap()).unwrap();
    fs::write(&target, format!("{}{}", &pending[..5], &old[5..])).unwrap();

    assert!(key(&mut a, "super+s"));
    assert_eq!(
        fs::read_to_string(&target).unwrap(),
        "live old revision text"
    );
    assert!(
        !a.workspace.tabs[a.workspace.active].dirty(),
        "{}",
        a.panels.message
    );
    assert!(!record.exists() && !stage.exists());
    assert!(
        a.workspace
            .resource_reports
            .iter()
            .any(|m| m.contains("Recovered interrupted save"))
            || a.panels.message.contains("Recovered interrupted save")
    );
}
#[test]
fn hot_exit_quits_without_loading_twenty_hidden_documents() {
    let root = tempfile::tempdir().unwrap();
    profile(root.path(), "{}");
    let mut launch = app(root.path());
    for i in 0..21 {
        let file = root.path().join(format!("document-{i}.txt"));
        fs::write(&file, i.to_string()).unwrap();
        launch.workspace.open(location(&file)).unwrap();
    }
    let (window, documents) = launch.workspace.snapshot().unwrap();
    drop(launch);
    let mut restored = app(root.path());
    restored
        .workspace
        .restore_window(&window, &documents)
        .unwrap();
    let count = |a: &App| {
        a.workspace
            .tabs
            .iter()
            .filter(|tab| tab.lazy.is_some())
            .count()
    };
    assert_eq!(count(&restored), 20);
    crate::restore_io::block_next();
    let started = Instant::now();
    assert!(restored.prepare_quit(None));
    assert!(
        started.elapsed() < Duration::from_millis(100),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(count(&restored), 20);
}

#[test]
fn lazy_restore_timeout_keeps_newer_checkpoint_and_launch_backup() {
    let root = tempfile::tempdir().unwrap();
    profile(root.path(), "{}");
    let mut first = app(root.path());
    for name in ["lazy.txt", "visible.txt"] {
        let file = root.path().join(name);
        fs::write(&file, name).unwrap();
        first.workspace.open(location(&file)).unwrap();
    }
    let (window, documents) = first.workspace.snapshot().unwrap();
    let path = root.path().join("Sessions/Session.sublime-session");
    let mut snapshot = vel_session::Session {
        windows: vec![window.clone()],
        documents: documents.clone(),
        ..Default::default()
    };
    vel_session::save(&path, &snapshot).unwrap();
    let launch = fs::read(&path).unwrap();
    drop(first);

    let mut a = app(root.path());
    a.workspace.restore_window(&window, &documents).unwrap();
    let lazy = a
        .workspace
        .tabs
        .iter()
        .position(|t| t.lazy.is_some())
        .unwrap();
    snapshot
        .recent_files
        .push(root.path().join("new-checkpoint"));
    vel_session::save(&path, &snapshot).unwrap();
    let newest = fs::read(&path).unwrap();
    crate::restore_io::block_next();
    let _ = a.workspace.prepare_sheet(lazy);
    assert_eq!(fs::read(&path).unwrap(), newest);
    assert!(
        path.parent()
            .unwrap()
            .read_dir()
            .unwrap()
            .flatten()
            .any(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .contains("restore-failed")
                    && fs::read(entry.path()).is_ok_and(|bytes| bytes == launch)
            })
    );
}

#[test]
fn shipped_language_resources_apply_indentation_extensions_and_snippets() {
    let root = tempfile::tempdir().unwrap();
    profile(root.path(), r#"{"translate_tabs_to_spaces":true}"#);
    fs::write(
        root.path().join("Packages/User/DOT.sublime-settings"),
        r#"{"tab_size":7}"#,
    )
    .unwrap();
    let mut a = app(root.path());
    a.workspace
        .open(location(&root.path().join("Makefile")))
        .unwrap();
    assert!(
        !a.workspace
            .editor()
            .settings
            .editor()
            .translate_tabs_to_spaces
    );
    assert!(key(&mut a, "tab"));
    assert_eq!(a.workspace.editor().text, "\t");
    for (file, syntax) in [("x.h", "C++"), ("graph.dot", "Graphviz (DOT)")] {
        a.workspace.open(location(&root.path().join(file))).unwrap();
        assert_eq!(
            a.workspace
                .editor()
                .language
                .borrow()
                .as_ref()
                .unwrap()
                .name,
            syntax
        );
    }
    assert_eq!(a.workspace.editor().settings.editor().tab_size, 7);
    a.workspace
        .open(location(&root.path().join("sample.py")))
        .unwrap();
    let snippet = a
        .workspace
        .editor()
        .snippets
        .keys()
        .find(|p| p.starts_with("Packages/Python/"))
        .unwrap()
        .clone();
    a.run_named(None, "insert_snippet", &json!({"name":snippet}));
    assert!(!a.workspace.editor().text.is_empty());

    profile(root.path(), r#"{"ignored_packages":["Python"]}"#);
    let mut ignored = app(root.path());
    ignored
        .workspace
        .open(location(&root.path().join("sample.py")))
        .unwrap();
    assert!(
        !ignored
            .workspace
            .editor()
            .snippets
            .keys()
            .any(|path| path.starts_with("Packages/Python/"))
    );
    assert!(!ignored.workspace.syntax_settings.contains_key("Python"));
}

#[test]
fn bundled_resource_inventory_is_visible_to_the_real_loader() {
    let mut resources = vel_config::resources::Resources::default();
    crate::shipped::register(&crate::language::packages_root(), &mut resources);
    for (suffix, minimum) in [
        (".sublime-snippet", 873),
        (".sublime-completions", 61),
        (".sublime-settings", 39),
        (".sublime-build", 17),
    ] {
        let count = resources.matching(suffix).len();
        println!("{suffix}: {count}");
        assert!(count >= minimum);
    }
}

fn loaded_snippets(a: &App) -> usize {
    a.workspace.catalog.snippets.values().filter(|entry| {
        matches!(entry, crate::completion::SnippetEntry::Shipped(snippet) if snippet.loaded())
    }).count()
}

#[test]
fn first_scoped_completion_and_tab_load_only_eligible_bodies() {
    for (extension, syntax, prefix, completion, tab, expansion) in [
        ("py", "Python", "_", "__", "__", "__init__"),
        ("html", "HTML", "<di", "div", "html", "<!DOCTYPE html>"),
    ] {
        for tab_trigger in [false, true] {
            let root = tempfile::tempdir().unwrap();
            profile(root.path(), "{}");
            let mut a = app(root.path());
            assert!(!a.workspace.syntax_settings.loaded(syntax));
            assert!(!a.workspace.build_systems.loaded());
            assert_eq!(loaded_snippets(&a), 0);
            let file = root.path().join(format!("example.{extension}"));
            let text = if tab_trigger { tab } else { prefix };
            fs::write(&file, text).unwrap();
            a.workspace.open(location(&file)).unwrap();
            a.panels.intel.poll(&mut a.workspace);
            a.workspace.editor_mut().set_caret(text.len(), false);
            if a.workspace.syntax_settings.contains_key(syntax) {
                assert!(a.workspace.syntax_settings.loaded(syntax));
            }
            assert_eq!(loaded_snippets(&a), 0);
            let deadline = Instant::now() + crate::test_timing::HANG_GUARD;
            while !a
                .workspace
                .editor()
                .language
                .borrow()
                .as_ref()
                .unwrap()
                .scopes_ready_at(&a.workspace.editor().text, text.len())
            {
                a.workspace.editor_mut().poll_language();
                assert!(Instant::now() < deadline, "{syntax} caret scope");
                std::thread::sleep(Duration::from_micros(100));
            }
            let start = Instant::now();
            if tab_trigger {
                assert!(key(&mut a, "tab"));
                assert!(
                    a.workspace
                        .editor()
                        .text
                        .to_string()
                        .to_ascii_lowercase()
                        .contains(&expansion.to_ascii_lowercase()),
                    "{syntax}: {}",
                    a.workspace.editor().text
                );
            } else {
                a.run_named(None, "auto_complete", &json!({}));
                while a.workspace.editor().completion.busy() {
                    a.workspace.editor_mut().poll_completion();
                    assert!(Instant::now() < deadline, "{syntax} manual completion");
                    std::thread::sleep(Duration::from_micros(100));
                }
                let list = a
                    .workspace
                    .editor()
                    .completion
                    .list
                    .as_ref()
                    .expect("completion popup");
                assert!(
                    list.items.iter().any(|item| item.trigger == completion),
                    "{syntax}: {:?}",
                    list.items
                );
            }
            let ms = start.elapsed().as_secs_f64() * 1000.;
            let bodies = loaded_snippets(&a);
            assert!(
                bodies < 100,
                "{syntax}: {bodies} snippet bodies materialized"
            );
            if tab_trigger {
                assert_eq!(bodies, 1, "{syntax}");
            }
            println!(
                "first {syntax} {}: {ms:.3} ms ({bodies} snippet bodies)",
                if tab_trigger { "Tab" } else { "completion" }
            );
        }
    }
}

#[test]
fn used_syntax_settings_reload_live_and_builds_wait_for_resolution() {
    let root = tempfile::tempdir().unwrap();
    profile(root.path(), "{}");
    let settings = root.path().join("Packages/User/Python.sublime-settings");
    fs::write(&settings, r#"{"tab_size":3}"#).unwrap();
    let mut a = app(root.path());
    assert!(!a.workspace.build_systems.loaded());
    a.workspace
        .open(location(&root.path().join("example.py")))
        .unwrap();
    assert_eq!(a.workspace.editor().preferences.tab_size, 3);
    fs::write(&settings, r#"{"tab_size":7}"#).unwrap();
    a.workspace.request_resource_reload();
    a.workspace.wait_resources();
    assert_eq!(a.workspace.editor().preferences.tab_size, 7);
    assert!(!a.workspace.build_systems.loaded());
    a.run_named(None, "show_build_systems", &json!({}));
    assert!(a.workspace.build_systems.loaded());
    assert!(
        a.workspace
            .build_systems
            .iter()
            .any(|build| build.name == "Python")
    );
}

#[cfg(unix)]
#[test]
fn linked_user_resources_and_writes_follow_alias_without_traversing_cycles() {
    use std::os::unix::fs::symlink;
    let root = tempfile::tempdir().unwrap();
    let external = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join("Packages")).unwrap();
    fs::write(
        external.path().join("Preferences.sublime-settings"),
        r#"{"tab_size":7}"#,
    )
    .unwrap();
    fs::write(
        external.path().join("Default.sublime-keymap"),
        r#"[{"keys":["super+j"],"command":"insert","args":{"characters":"linked"}}]"#,
    )
    .unwrap();
    symlink(external.path(), root.path().join("Packages/User")).unwrap();
    symlink("..", root.path().join("Packages/Loop")).unwrap();
    let mut a = app(root.path());
    assert_eq!(a.workspace.editor().settings.editor().tab_size, 7);
    assert!(key(&mut a, "super+j"));
    assert_eq!(a.workspace.editor().text, "linked");
    vel_packages::PackageManager::new(root.path())
        .set_enabled("Python", false)
        .unwrap();
    assert!(
        fs::read_to_string(external.path().join("Preferences.sublime-settings"))
            .unwrap()
            .contains("Python")
    );
    assert!(
        root.path()
            .join("Packages/User")
            .symlink_metadata()
            .unwrap()
            .is_symlink()
    );
}

#[test]
fn package_keymaps_skip_python_and_bad_entries_and_report_once() {
    let root = tempfile::tempdir().unwrap();
    profile(root.path(), "{}");
    let package = root.path().join("Packages/Fixture");
    fs::create_dir(&package).unwrap();
    fs::write(package.join("plugin.py"), "raise Exception('not executed')").unwrap();
    fs::write(
        package.join("Default.sublime-keymap"),
        r#"[
        {"keys":["super+d"],"command":"python_only"},
        {"keys":["garbage+j"],"command":"insert"},
        {"keys":["super+j"],"command":"insert","args":{"characters":"ok"}}
    ]"#,
    )
    .unwrap();
    fs::write(
        package.join("Example.sublime-keymap"),
        r#"[{"keys":["super+k"],"command":"insert","args":{"characters":"ignored"}}]"#,
    )
    .unwrap();
    let mut a = app(root.path());
    a.run_named(None, "insert", &json!({"characters":"one one"}));
    a.workspace.editor_mut().set_caret(1, false);
    assert!(key(&mut a, "super+d"));
    assert!(key(&mut a, "super+d"));
    assert_eq!(a.workspace.editor().regions().len(), 2);
    assert!(key(&mut a, "super+j"));
    assert!(a.workspace.editor().text.to_string().contains("ok"));
    assert!(!key(&mut a, "super+k") || !a.workspace.editor().text.to_string().contains("ignored"));
    assert!(a.panels.message.contains("Fixture"));
    assert!(a.workspace.take_resource_reports().is_empty());
}

#[test]
fn repaired_keymap_applies_by_polling_the_invalid_origin() {
    let root = tempfile::tempdir().unwrap();
    profile(root.path(), "{}");
    let path = root.path().join("Packages/User/Default.sublime-keymap");
    fs::write(&path, "[").unwrap();
    let mut a = app(root.path());
    assert!(!key(&mut a, "super+alt+j"));
    fs::write(
        &path,
        r#"[{"keys":["super+alt+j"],"command":"insert","args":{"characters":"repaired"}}]"#,
    )
    .unwrap();
    let deadline = Instant::now() + crate::test_timing::HANG_GUARD;
    loop {
        a.workspace.poll();
        if key(&mut a, "super+alt+j") {
            break;
        }
        assert!(Instant::now() < deadline, "{:?}", a.workspace.messages);
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(a.workspace.editor().text, "repaired");
}

// Round 22 review: drive the effective keymap, settings menus and syntax loader.
#[test]
fn python_enter_uses_the_shipped_dedent_macro_and_raise_keeps_normal_enter() {
    let root = tempfile::tempdir().unwrap();
    profile(root.path(), "{}");
    let mut a = app(root.path());
    for (i, line) in [
        "return x",
        "pass",
        "break",
        "continue",
        "raise ValueError()",
    ]
    .into_iter()
    .enumerate()
    {
        let file = root.path().join(format!("a{i}.py"));
        let text = format!("def f(x):\n    {line}");
        fs::write(&file, &text).unwrap();
        a.workspace.open(location(&file)).unwrap();
        a.workspace.editor_mut().set_caret(text.len(), false);
        let deadline = Instant::now() + crate::test_timing::HANG_GUARD;
        while !a
            .workspace
            .editor()
            .language
            .borrow()
            .as_ref()
            .unwrap()
            .scopes_ready_at(&a.workspace.editor().text, text.len())
        {
            a.workspace.editor_mut().poll_language();
            assert!(Instant::now() < deadline, "{line} scope");
            std::thread::sleep(Duration::from_micros(100));
        }
        assert!(key(&mut a, "enter"));
        a.handle_text_input(
            None,
            TextInput::Insert {
                text: "y".into(),
                replacement: None,
            },
        );
        let indent = if line.starts_with("raise") {
            "    "
        } else {
            ""
        };
        assert_eq!(
            a.workspace.editor().text.to_string(),
            format!("{text}\n{indent}y"),
            "{line}: {}",
            a.panels.message
        );
        assert!(
            !a.panels.message.contains("not available"),
            "{}",
            a.panels.message
        );
    }
}

#[test]
fn shipped_keymap_sweep_dispatches_every_loaded_binding() {
    use vel_config::keymap::Keymap;
    let root = tempfile::tempdir().unwrap();
    profile(root.path(), "{}");
    let mut a = app(root.path());
    let bindings = a
        .workspace
        .editor()
        .keymap
        .bindings()
        .iter()
        .filter(|b| {
            b.origin.as_ref().is_some_and(|path| {
                !path.starts_with("Packages/Default/")
                    && a.workspace.resources.get(path).is_some_and(|r| r.shipped())
            })
        })
        .cloned()
        .collect::<Vec<_>>();
    for package in ["Python", "Groovy", "Java", "TOML"] {
        assert_eq!(
            bindings
                .iter()
                .filter(|binding| binding.invocation.command == "chain"
                    && binding
                        .origin
                        .as_ref()
                        .unwrap()
                        .starts_with(&format!("Packages/{package}/")))
                .count(),
            2,
            "{package} triple-quote bindings"
        );
    }
    assert!(
        bindings.len() >= 121,
        "only {} shipped bindings",
        bindings.len()
    );
    let mut opened = std::collections::BTreeSet::new();
    for mut binding in bindings {
        let source = binding.origin.as_ref().unwrap().clone();
        let package = source.split('/').nth(1).unwrap();
        let syntax = a
            .workspace
            .assets
            .syntaxes
            .syntaxes()
            .iter()
            .find(|s| {
                s.path.starts_with(&format!("{package}/"))
                    || s.path.starts_with(&format!("Packages/{package}/"))
            })
            .unwrap_or_else(|| panic!("no grammar for {package}"))
            .path
            .clone();
        if opened.insert(package.to_owned()) {
            a.workspace.new_file().unwrap();
            a.workspace.set_file_type(&syntax).unwrap();
        }
        a.panels.message.clear();
        // Contexts constrain when a binding runs; clear them in the isolated
        // map so even mutually exclusive contexts exercise their invocation.
        binding.context.clear();
        let keys = binding.keys.clone();
        let command = binding.invocation.command.clone();
        a.workspace.editor_mut().keymap = Keymap::from_bindings(vec![binding]);
        for key in keys {
            assert!(
                a.dispatch_key_binding(None, &key, Instant::now(), a.viewport()),
                "{source}: {command} ({key:?})"
            );
        }
        assert!(
            !a.panels.message.contains("not available")
                && !a.panels.message.contains("unknown command")
                && !a.panels.message.contains("chain entry"),
            "{source}: {command}: {}",
            a.panels.message
        );
    }
    assert!(opened.len() >= 15, "{opened:?}");
}

#[test]
fn object_chain_dispatches_user_keys_and_shipped_python_triple_quotes() {
    let root = tempfile::tempdir().unwrap();
    profile(root.path(), "{}");
    fs::write(
        root.path()
            .join("Packages/User/Default (OSX).sublime-keymap"),
        r#"[
        {"keys":["super+j"],"command":"chain","args":{"commands":[
            {"command":"insert","args":{"characters":"object "}},
            ["insert",{"characters":"array"}]
        ]}}
    ]"#,
    )
    .unwrap();
    let mut a = app(root.path());
    assert!(key(&mut a, "super+j"));
    assert_eq!(a.workspace.editor().text, "object array");
    for quote in ['"', '\''] {
        let text = format!("x = {}", quote.to_string().repeat(6));
        let file = root.path().join(format!("{quote}.py"));
        fs::write(&file, text).unwrap();
        a.open_received_paths(&[file]).unwrap();
        let caret = 7;
        a.workspace.editor_mut().set_caret(caret, false);
        let deadline = Instant::now() + crate::test_timing::HANG_GUARD;
        loop {
            let editor = a.workspace.editor_mut();
            editor.poll_language();
            if editor
                .language
                .borrow()
                .as_ref()
                .unwrap()
                .scopes_ready_at(&editor.text, caret)
            {
                break;
            }
            assert!(Instant::now() < deadline, "Python scopes did not load");
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(key(&mut a, "backspace"));
        assert_eq!(
            a.workspace.editor().text.to_string(),
            format!("x = {quote}{quote}")
        );
        assert_eq!(a.workspace.editor().caret, 5);
    }
}

#[test]
fn project_hot_exit_rotates_only_in_the_profile_and_restores_the_previous_text() {
    let root = tempfile::tempdir().unwrap();
    let external = tempfile::tempdir().unwrap();
    profile(root.path(), "{}");
    let project = external.path().join("test.sublime-project");
    fs::write(&project, "{}").unwrap();
    let mut a = app(root.path());
    a.workspace.open_project(project.clone()).unwrap();
    for text in ["unsaved text", " on the next checkpoint"] {
        a.run_named(None, "insert", &json!({"characters":text}));
        a.checkpoint_reason(false, false).unwrap();
        a.sessions.borrow().flush().unwrap();
    }
    let primary = vel_session::project::workspace_path(&project);
    let mut files: Vec<_> = fs::read_dir(external.path())
        .unwrap()
        .flatten()
        .map(|e| e.file_name())
        .collect();
    files.sort();
    assert_eq!(files, ["test.sublime-project", "test.sublime-workspace"]);
    fs::write(&primary, "truncated workspace").unwrap();
    let recovery = vel_session::load_project_workspace(&project, Some(root.path()));
    assert!(
        recovery
            .source
            .as_ref()
            .unwrap()
            .starts_with(root.path().join("Sessions"))
    );
    assert!(!recovery.messages.is_empty());
    let mut reopened = app(root.path());
    reopened.workspace.open_project(project).unwrap();
    reopened.workspace.materialize_input();
    assert_eq!(reopened.workspace.editor().text, "unsaved text");
    assert!(reopened.workspace.tabs[reopened.workspace.active].dirty());
}

#[test]
fn ignored_keymap_notice_repeats_only_after_that_file_changes() {
    let root = tempfile::tempdir().unwrap();
    profile(root.path(), "{}");
    let user = root.path().join("Packages/User");
    let other = user.join("Unrecognized.sublime-keymap");
    for file in [
        other.clone(),
        user.join("Default (Linux).sublime-keymap"),
        user.join("Default (Windows).sublime-keymap"),
    ] {
        fs::write(file, "[]").unwrap();
    }
    let mut a = app(root.path());
    let ignored = |a: &mut App| {
        a.workspace
            .take_resource_reports()
            .into_iter()
            .filter(|r| r.starts_with("Ignored Packages/User/"))
            .collect::<Vec<_>>()
    };
    let mut first = ignored(&mut a);
    if a.panels.message.starts_with("Ignored Packages/User/") {
        first.push(std::mem::take(&mut a.panels.message).to_string());
    }
    assert_eq!(first.len(), 1, "{first:?}");
    assert!(first[0].contains("Unrecognized"));
    for changed in [false, true, false] {
        if changed {
            fs::write(&other, "[\n]").unwrap();
        }
        a.workspace.request_resource_reload();
        a.workspace.wait_resources();
        assert_eq!(ignored(&mut a).len(), usize::from(changed));
    }
}

#[test]
fn missing_package_targets_do_not_shadow_other_bindings() {
    let root = tempfile::tempdir().unwrap();
    profile(root.path(), "{}");
    let dir = root.path().join("Packages/Fixture");
    fs::create_dir(&dir).unwrap();
    let replacement = root.path().join("Packages/Default");
    fs::create_dir(&replacement).unwrap();
    fs::write(
        replacement.join("Velocity.sublime-keymap"),
        r#"[{"keys":["super+alt+k"],"command":"run_macro_file","args":{"file":"Packages/Fixture/missing.sublime-macro"}}]"#,
    ).unwrap();
    fs::write(dir.join("Default.sublime-keymap"), r#"[
        {"keys":["super+alt+j"],"command":"insert","args":{"characters":"works"}},
        {"keys":["super+alt+j"],"command":"run_macro_file","args":{"file":"Packages/Fixture/missing.sublime-macro"}},
        {"keys":["super+alt+j"],"command":"insert_snippet","args":{"name":"Packages/Fixture/missing.sublime-snippet"}},
        {"keys":["super+alt+j"],"command":"edit_settings","args":{"base_file":"Packages/Fixture/missing.sublime-settings"}},
        {"keys":["super+alt+j"],"command":"run_macro_file","args":{"file":"Packages/Fixture/Broken.sublime-macro"}}
    ]"#).unwrap();
    fs::write(
        dir.join("Broken.sublime-macro"),
        r#"[{"command":"unported_python"}]"#,
    )
    .unwrap();
    let mut a = app(root.path());
    assert!(!key(&mut a, "super+alt+k"));
    assert!(key(&mut a, "super+alt+j"));
    assert_eq!(a.workspace.editor().text, "works");
}

#[test]
fn first_save_and_save_as_publish_complete_new_files() {
    let root = tempfile::tempdir().unwrap();
    profile(root.path(), "{}");
    let mut a = app(root.path());
    a.run_named(None, "insert", &json!({"characters":"new file"}));
    let first = root.path().join("first.txt");
    a.run_named(None, "save", &json!({"file":first}));
    assert_eq!(fs::read_to_string(&first).unwrap(), "new file");
    a.run_named(None, "insert", &json!({"characters":" as"}));
    let second = root.path().join("second.txt");
    a.run_named(None, "save_as", &json!({"file":second}));
    assert_eq!(fs::read_to_string(first).unwrap(), "new file");
    assert_eq!(fs::read_to_string(second).unwrap(), "new file as");
    assert!(!a.workspace.tabs[a.workspace.active].dirty());
}

#[test]
fn preferences_key_bindings_creates_the_loaded_user_file_and_keeps_legacy() {
    let root = tempfile::tempdir().unwrap();
    profile(root.path(), "{}");
    let user = root.path().join("Packages/User");
    fs::write(
        user.join("Velocity.sublime-keymap"),
        r#"[
        {"keys":["super+d"],"command":"insert","args":{"characters":"legacy"}},
        {"keys":["super+j"],"command":"insert","args":{"characters":"legacy-only"}}
    ]"#,
    )
    .unwrap();
    fs::write(user.join("Other.sublime-keymap"), "[]").unwrap();
    let mut a = app(root.path());
    let menu = a
        .workspace
        .menus
        .iter()
        .flat_map(|item| &item.children)
        .find(|item| item.caption == "Key Bindings…")
        .unwrap()
        .clone();
    let palette = a
        .workspace
        .palette
        .iter()
        .find(|item| item.caption == "Preferences: Key Bindings…")
        .unwrap();
    assert_eq!(menu.args, palette.invocation.args);
    a.run_named(None, menu.command.as_ref().unwrap(), &menu.args);
    let canonical = user.join("Default (OSX).sublime-keymap");
    assert_eq!(
        a.opened_settings
            .last()
            .unwrap()
            .workspace
            .tabs
            .last()
            .unwrap()
            .path
            .as_ref(),
        Some(&canonical.canonicalize().unwrap())
    );
    assert!(canonical.is_file(), "{}", a.panels.message);
    assert!(key(&mut a, "super+d"));
    assert_eq!(a.workspace.editor().text, "legacy");
    fs::write(
        &canonical,
        r#"[{"keys":["super+d"],"command":"insert","args":{"characters":"canonical"}}]"#,
    )
    .unwrap();
    a.workspace.request_resource_reload();
    a.workspace.wait_resources();
    assert!(key(&mut a, "super+d") && key(&mut a, "super+j"));
    assert_eq!(a.workspace.editor().text, "legacycanonicallegacy-only");
    assert!(
        a.panels.message.contains("Other.sublime-keymap")
            || a.workspace
                .take_resource_reports()
                .iter()
                .any(|r| r.contains("Other.sublime-keymap"))
    );
}

#[test]
fn syntax_display_name_overrides_survive_shipped_settings_and_an_empty_stem() {
    let root = tempfile::tempdir().unwrap();
    profile(root.path(), "{}");
    let user = root.path().join("Packages/User");
    let display = user.join("Java Properties.sublime-settings");
    let stem = user.join("JavaProperties.sublime-settings");
    fs::write(&display, r#"{"tab_size":5}"#).unwrap();
    fs::write(
        user.join("Graphviz (DOT).sublime-settings"),
        r#"{"tab_size":7}"#,
    )
    .unwrap();
    let mut a = app(root.path());
    for (syntax, size) in [("Graphviz (DOT)", 7), ("Java Properties", 5)] {
        a.workspace.set_file_type(syntax).unwrap();
        assert_eq!(a.workspace.editor().settings.editor().tab_size, size);
    }
    a.run_named(None, "edit_syntax_settings", &json!({}));
    assert!(!stem.exists());
    assert_eq!(
        a.opened_settings
            .last()
            .unwrap()
            .workspace
            .tabs
            .last()
            .unwrap()
            .path
            .as_ref(),
        Some(&display.canonicalize().unwrap())
    );
    fs::write(&stem, "{}").unwrap();
    a.workspace.request_resource_reload();
    a.workspace.wait_resources();
    assert_eq!(a.workspace.editor().settings.editor().tab_size, 5);
    fs::write(&stem, r#"{"tab_size":9}"#).unwrap();
    a.workspace.request_resource_reload();
    a.workspace.wait_resources();
    assert_eq!(a.workspace.editor().settings.editor().tab_size, 9);
}

#[test]
fn registry_includes_every_named_dispatch_route_before_package_filtering() {
    let registry = crate::editor::Editor::command_registry();
    for command in crate::editor::DISPATCHED_WINDOW_COMMANDS
        .iter()
        .copied()
        .chain([
            "toggle_tabs",
            "toggle_status_bar",
            "reopen_last_file",
            "clear_recent_items",
            "new_build_system",
        ])
    {
        assert!(registry.kind(command).is_some(), "{command}");
    }
}
