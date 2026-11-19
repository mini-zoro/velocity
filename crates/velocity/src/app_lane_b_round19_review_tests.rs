//! Review regressions through App commands, key/mouse dispatch and real session/loaders.
use super::*;
use std::collections::BTreeMap;

fn set(a: &mut App, setting: &str, value: Value) {
    named(a, "set_setting", json!({"setting":setting,"value":value}));
}
fn caret_visible(a: &mut App) {
    a.scene();
    let e = a.workspace.editor();
    let byte = e.buffer.view(e.id).sel().primary().unwrap().b;
    let row = e.layout.borrow().visual_at(&e.text, byte).0;
    let y = row as f64 * a.view.line_height;
    assert!(
        y >= a.view.scroll_y - 1.
            && y < a.view.scroll_y + (e.layout.borrow().page as f64 * a.view.line_height),
        "caret row {row}, scroll {}, page {}",
        a.view.scroll_y,
        (e.layout.borrow().page as f64 * a.view.line_height)
    );
}
#[test]
fn r19_review_vintage_pending_motions_and_search_reveal() {
    let mut a = app(None);
    let text = (0..3000)
        .map(|i| {
            format!(
                "row {i} {}\n",
                if i == 500 || i == 2400 {
                    "needle"
                } else {
                    "text"
                }
            )
        })
        .collect::<String>();
    named(&mut a, "select_all", json!({}));
    named(&mut a, "insert", json!({"characters":text}));
    named(&mut a, "move_to", json!({"to":"bof"}));
    set(&mut a, "animation_enabled", json!(false));
    set(&mut a, "vintage_start_in_command_mode", json!(true));
    set(&mut a, "vintage_enabled", json!(true));
    key(&mut a, "escape");
    a.scene();
    key(&mut a, "shift+g");
    assert!(a.view.scroll_y > 30_000.);
    caret_visible(&mut a);
    key(&mut a, "g");
    key(&mut a, "g");
    assert!(a.view.scroll_y < 1.);
    caret_visible(&mut a);
    for _ in 0..80 {
        key(&mut a, "j");
        caret_visible(&mut a);
    }
    assert!(a.view.scroll_y > 0.);
    key(&mut a, "shift+g");
    // Simulate a wheel-parked viewport before the operator edits near EOF.
    key(&mut a, "k");
    a.view.scroll_y = 0.;
    key(&mut a, "d");
    key(&mut a, "d");
    caret_visible(&mut a);
    key(&mut a, "g");
    key(&mut a, "g");
    key(&mut a, "/");
    let viewport = a.viewport();
    assert!(a.panels.text_input(
        &mut a.workspace,
        TextInput::Insert {
            text: "needle".into(),
            replacement: None,
        },
        viewport
    ));
    a.changed_state(Instant::now(), false);
    key(&mut a, "enter");
    caret_visible(&mut a);
    let first = a.view.scroll_y;
    let first_row = a
        .workspace
        .editor()
        .text
        .byte_to_line(a.workspace.editor().caret);
    assert_eq!(
        first_row,
        500,
        "first search, scroll {first}, mode {}",
        a.workspace.editor().vintage_status()
    );
    key(&mut a, "n");
    caret_visible(&mut a);
    assert_eq!(
        a.workspace
            .editor()
            .text
            .byte_to_line(a.workspace.editor().caret),
        2400
    );
    assert!(
        a.view.scroll_y > first + 20_000.,
        "first {first}, next {}",
        a.view.scroll_y
    );
}
#[test]
fn r19_review_reducing_layout_repairs_empty_receiver_and_keeps_single_selection() {
    let mut a = app(None);
    for binding in ["super+alt+3", "ctrl+shift+3", "ctrl+1", "super+alt+2"] {
        assert!(key(&mut a, binding));
        validate(&a);
    }
    let (window, documents) = a.workspace.snapshot().unwrap();
    vel_session::Session {
        windows: vec![window],
        documents,
        ..Default::default()
    }
    .validate()
    .unwrap();
    named(&mut a, "focus_group", json!({"group":1}));
    let receiver = a.workspace.workbench.groups[0].focused;
    key(&mut a, "super+alt+1");
    validate(&a);
    assert_eq!(a.workspace.workbench.groups[0].focused, receiver);
    assert_eq!(a.workspace.workbench.groups[0].selected.len(), 1);
    a.workspace.workbench.groups[0].anchor = Some(u64::MAX);
    a.checkpoint(false).unwrap();
    assert_eq!(
        a.workspace.workbench.groups[0].anchor,
        a.workspace.workbench.groups[0].focused
    );
}
#[test]
fn r19_review_single_close_and_middle_click_preserve_other_selected_sheets() {
    for close in ["close", "close_file", "middle"] {
        let mut a = app(None);
        for _ in 0..3 {
            named(&mut a, "new_file", json!({}));
        }
        named(&mut a, "select_all_tabs", json!({}));
        let sheets = ids(&a);
        let closed = if close == "middle" {
            a.scene();
            let (id, rect) = a.tab_hits[0];
            a.handle_mouse_event(
                None,
                MouseEvent {
                    kind: MouseKind::Down,
                    position: Point {
                        x: rect.x + 8.,
                        y: rect.y + 8.,
                    },
                    button: 2,
                    click_count: 1,
                    modifiers: Modifiers(0),
                    timestamp: 0.,
                },
            );
            id
        } else {
            let id = a.workspace.tabs[a.workspace.active].sheet;
            if close == "close" {
                assert!(key(&mut a, "super+w"));
            } else {
                named(&mut a, close, json!({}));
            }
            id
        };
        let remaining = sheets.into_iter().filter(|id| *id != closed).collect();
        assert_eq!(ids(&a), remaining);
        assert_eq!(a.workspace.workbench.groups[0].selected, remaining);
        assert_eq!(a.workspace.workbench.groups[0].visible().len(), 3);
        assert_eq!(
            a.workspace.workbench.groups[0].focused,
            remaining.last().copied()
        );
        validate(&a);
    }
}
#[test]
fn r19_review_split_close_never_prompts_or_discards_original() {
    for dirty in [false, true] {
        let mut a = app(None);
        let original = a.workspace.tabs[0].sheet;
        if dirty {
            named(&mut a, "insert", json!({"characters":"unsaved"}));
        }
        let text = a.workspace.editor().text.to_string();
        named(&mut a, "split_view", json!({}));
        a.close_decisions
            .push_back(tab_adapter::CloseDecision::Cancel);
        assert!(key(&mut a, "super+w"));
        assert_eq!(ids(&a), [original].into_iter().collect());
        assert_eq!(a.workspace.editor().text.to_string(), text);
        assert_eq!(a.close_decisions.len(), 1);
        validate(&a);
    }
}
#[test]
fn r19_review_clone_is_single_selection_split_is_multiple_and_empty_is_atomic() {
    let mut a = app(None);
    let original = a.workspace.tabs[0].sheet;
    named(&mut a, "clone_file", json!({}));
    let clone = a.workspace.tabs[a.workspace.active].sheet;
    assert_eq!(a.workspace.workbench.groups[0].visible(), vec![clone]);
    key(&mut a, "super+alt+2");
    key(&mut a, "ctrl+shift+2");
    assert_eq!(a.workspace.workbench.groups[0].sheets, vec![original]);
    assert_eq!(a.workspace.workbench.groups[1].sheets, vec![clone]);
    named(&mut a, "split_view", json!({}));
    assert_eq!(a.workspace.workbench.groups[1].selected.len(), 2);
    key(&mut a, "ctrl+shift+1");
    assert_eq!(a.workspace.workbench.groups[1].sheets, vec![clone]);
    for command in ["split_view", "clone_file"] {
        while !a.workspace.is_empty() {
            key(&mut a, "super+w");
        }
        assert!(a.workspace.is_empty());
        named(&mut a, command, json!({}));
        assert_eq!(a.workspace.tabs.len(), 1);
        validate(&a);
        let (window, documents) = a.workspace.snapshot().unwrap();
        vel_session::Session {
            windows: vec![window],
            documents,
            ..Default::default()
        }
        .validate()
        .unwrap();
    }
}
#[test]
fn r19_review_grid_close_sends_tabs_to_absorbing_cell() {
    let mut a = app(None);
    key(&mut a, "super+alt+5");
    named(&mut a, "focus_group", json!({"group":2}));
    let sheet = a.workspace.tabs[a.workspace.active].sheet;
    let top_left = a.workspace.workbench.layout.cells[0];
    named(&mut a, "close_pane", json!({}));
    assert_eq!(a.workspace.workbench.group_of(sheet), Some(0));
    let grown = a.workspace.workbench.layout.cells[0];
    assert_eq!(grown[0], top_left[0]);
    assert!(grown[3] > top_left[3]);
    validate(&a);
}
#[test]
fn r19_review_quit_salvages_dirty_documents_and_other_windows() {
    let data = tempfile::tempdir().unwrap();
    let mut first = app(Some(data.path()));
    let mut other = app(Some(data.path()));
    other.sessions = first.sessions.clone();
    other.session_id = other.sessions.borrow_mut().register();
    named(
        &mut other,
        "insert",
        json!({"characters":"other window dirty"}),
    );
    other.checkpoint(false).unwrap();
    named(&mut first, "insert", json!({"characters":"repair me"}));
    let sheet = first.workspace.tabs[0].sheet;
    first.workspace.workbench.groups[0].sheets = vec![u64::MAX];
    first.workspace.workbench.groups[0].focused = None;
    first.workspace.workbench.groups[0].selected = [sheet, u64::MAX].into_iter().collect();
    // Force the layout-free fallback too, rather than just the normalizer.
    first.view.scroll_x = f64::NAN;
    assert!(first.prepare_quit(None));
    assert!(first.workspace.messages.is_empty());
    let recovery = vel_session::load(&data.path().join("Sessions/Session.sublime-session"));
    assert!(recovery.messages.is_empty(), "{:?}", recovery.messages);
    recovery.session.validate().unwrap();
    assert_eq!(recovery.session.windows.len(), 2);
    for expected in ["repair me", "other window dirty"] {
        assert!(recovery.session.documents.iter().any(|d| {
            d.history
                .as_ref()
                .is_some_and(|h| h.current_text().unwrap().to_string().starts_with(expected))
        }));
    }
}
#[test]
fn r19_review_settings_identity_syntax_readonly_survive_real_session() {
    let data = tempfile::tempdir().unwrap();
    let mut a = app(Some(data.path()));
    key(&mut a, "super+,");
    let mut settings = a.opened_settings.pop().unwrap();
    settings.save_workspace_checkpoint().unwrap();
    let session = vel_session::load(&data.path().join("Sessions/Session.sublime-session")).session;
    session.validate().unwrap();
    let mut restored = app(Some(data.path()));
    restored
        .workspace
        .restore_window(&session.windows[0], &session.documents)
        .unwrap();
    let left = restored
        .workspace
        .sheet_index(restored.workspace.workbench.groups[0].focused.unwrap())
        .unwrap();
    let right = restored
        .workspace
        .sheet_index(restored.workspace.workbench.groups[1].focused.unwrap())
        .unwrap();
    assert!(restored.workspace.tabs[left].editor.read_only);
    assert!(restored.workspace.tabs[left].label().ends_with("(Default)"));
    assert!(restored.workspace.tabs[right].label().ends_with("(User)"));
    assert_eq!(
        restored.workspace.tabs[left]
            .editor
            .language
            .borrow()
            .as_ref()
            .unwrap()
            .name,
        "JSON"
    );
    assert_eq!(
        restored.workspace.tabs[right]
            .editor
            .language
            .borrow()
            .as_ref()
            .unwrap()
            .name,
        "JSON"
    );
    named(&mut restored, "focus_group", json!({"group":0}));
    let text = restored.workspace.editor().text.to_string();
    named(
        &mut restored,
        "insert",
        json!({"characters":"must not edit"}),
    );
    assert_eq!(restored.workspace.editor().text.to_string(), text);
    assert!(!restored.workspace.tabs[left].dirty());
}
#[test]
fn r19_review_open_dir_expands_window_variables_without_creating_paths() {
    let data = tempfile::tempdir().unwrap();
    let file = data.path().join("test.txt");
    std::fs::write(&file, "text").unwrap();
    let mut a = app(Some(data.path()));
    named(&mut a, "open_file", json!({"file":file}));
    named(&mut a, "add_folder", json!({"dir":data.path()}));
    for variable in ["$file_path", "${folder}"] {
        named(&mut a, "open_dir", json!({"dir":variable}));
        assert_eq!(
            a.revealed_directory
                .as_ref()
                .unwrap()
                .canonicalize()
                .unwrap(),
            data.path().canonicalize().unwrap()
        );
    }
    let missing = data.path().join("does-not-exist");
    a.revealed_directory = None;
    named(
        &mut a,
        "open_dir",
        json!({"dir":"${file_path}/does-not-exist"}),
    );
    assert!(!missing.exists());
    assert!(a.revealed_directory.is_none());
    named(&mut a, "open_dir", json!({"dir":"$unknown_variable"}));
    assert!(a.revealed_directory.is_none());
    assert!(!Path::new("$file_path").exists());
}
#[test]
fn r19_review_unmodified_uses_owner_and_deleted_checks_contents() {
    let data = tempfile::tempdir().unwrap();
    let mut a = app(Some(data.path()));
    named(&mut a, "clone_file", json!({}));
    named(&mut a, "insert", json!({"characters":"owner dirty"}));
    assert!(!a.command_enabled_args("close_unmodified", &json!({})));
    for (name, dirty, deleted) in [
        ("sidebar", false, true),
        ("external", false, false),
        ("edited", true, true),
    ] {
        let path = data.path().join(name);
        std::fs::write(&path, "disk").unwrap();
        named(&mut a, "open_file", json!({"file":path}));
        if dirty {
            named(&mut a, "insert", json!({"characters":"edit"}));
        }
        a.workspace.tabs[a.workspace.active].deleted = deleted;
        std::fs::remove_file(&path).unwrap();
    }
    a.close_decisions
        .push_back(tab_adapter::CloseDecision::Cancel);
    named(&mut a, "close_deleted_files", json!({}));
    assert_eq!(
        a.workspace.tabs.iter().filter(|t| t.path.is_some()).count(),
        3
    );
    a.close_decisions
        .push_back(tab_adapter::CloseDecision::Discard);
    named(&mut a, "close_deleted_files", json!({}));
    assert!(a.workspace.tabs.iter().all(|t| t.path.is_none()));
    assert!(a.close_decisions.is_empty());
    validate(&a);
}
#[test]
fn r19_review_close_save_failure_uses_shared_decision_and_save_path() {
    let data = tempfile::tempdir().unwrap();
    let mut a = app(None);
    named(&mut a, "insert", json!({"characters":"keep me"}));
    let sheets = ids(&a);
    a.close_decisions
        .push_back(tab_adapter::CloseDecision::Save(Some(
            data.path().to_owned(),
        )));
    key(&mut a, "super+w");
    assert_eq!(ids(&a), sheets);
    let saved = data.path().join("saved.txt");
    a.close_decisions
        .push_back(tab_adapter::CloseDecision::Save(Some(saved.clone())));
    key(&mut a, "super+w");
    assert!(a.workspace.is_empty());
    assert!(
        std::fs::read_to_string(saved)
            .unwrap()
            .starts_with("keep me")
    );
}
fn sweep(seeds: u64, steps: usize) {
    let presets = [
        "single", "columns2", "columns3", "columns4", "rows2", "rows3", "grid4", "custom",
    ];
    for seed in 0..seeds {
        let mut a = app(None);
        let mut rng = seed + 0x19b;
        for step in 0..steps {
            rng = rng
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let before = ids(&a);
            let identities = a
                .workspace
                .tabs
                .iter()
                .map(|t| (t.sheet, t.document))
                .collect::<BTreeMap<_, _>>();
            let count = a.workspace.workbench.groups.len();
            let action = (rng >> 32) % 12;
            let mut permits_creation = false;
            match action {
                0 | 1 => {
                    let preset = presets[(rng as usize >> 8) % presets.len()];
                    permits_creation = true; // Focusing an empty group creates an untitled document.
                    let args = if preset == "custom" {
                        json!({"cols":[0.,0.25,1.],"rows":[0.,0.5,1.],"cells":[[0,0,1,2],[1,0,2,1],[1,1,2,2]]})
                    } else {
                        json!({"preset":preset})
                    };
                    named(&mut a, "set_layout", args);
                }
                2 => {
                    permits_creation = true;
                    named(&mut a, "split_view", json!({}));
                }
                3 => {
                    permits_creation = true;
                    named(&mut a, "clone_file", json!({}));
                }
                4 if count < 12 => named(&mut a, "new_pane", json!({"move":rng & 32 == 0})),
                5 if count > 1 => {
                    permits_creation = true;
                    named(&mut a, "close_pane", json!({}));
                }
                6 => {
                    permits_creation = true;
                    named(
                        &mut a,
                        "focus_group",
                        json!({"group":(rng as usize >> 8)%count}),
                    );
                }
                7 => named(
                    &mut a,
                    "move_to_group",
                    json!({"group":(rng as usize >> 8)%count}),
                ),
                8 => named(&mut a, "select_all_tabs", json!({})),
                9 | 10 => {
                    let active = a.workspace.tabs[a.workspace.active].sheet;
                    named(&mut a, "close", json!({"force":true}));
                    let after = ids(&a);
                    assert_eq!(
                        after,
                        before.into_iter().filter(|id| *id != active).collect(),
                        "seed {seed} step {step}"
                    );
                    validate(&a);
                    continue;
                }
                _ => {
                    permits_creation = true;
                    named(&mut a, "new_file", json!({}));
                }
            }
            let after = ids(&a);
            assert!(before.is_subset(&after), "lost tab seed {seed} step {step}");
            if !permits_creation {
                assert_eq!(before, after, "seed {seed} step {step}");
            }
            for tab in &a.workspace.tabs {
                if let Some(doc) = identities.get(&tab.sheet) {
                    assert_eq!(tab.document, *doc);
                }
            }
            validate(&a);
            if step % 29 == 0 {
                let (window, documents) = a.workspace.snapshot().unwrap();
                vel_session::Session {
                    windows: vec![window],
                    documents,
                    ..Default::default()
                }
                .validate()
                .unwrap();
            }
        }
    }
}
#[test]
fn r19_review_layout_state_machine_with_layouts_splits_empty_groups_and_closes() {
    sweep(8, 160);
}
#[test]
#[ignore = "extended review sweep: 40 seeds x 500 App-path operations"]
fn r19_review_extended_layout_state_machine_40x500() {
    sweep(40, 500);
}

#[test]
fn r19_review_scene_initializes_native_font_metrics() {
    let mut a = app(None);
    a.scene();
    assert!(a.workspace.themes.ui.borrow().metrics.native_enabled());
}
#[test]
fn r19_review_settings_errors_retain_each_file_and_other_valid_changes_apply() {
    let data = tempfile::tempdir().unwrap();
    let user = data.path().join("Packages/User");
    std::fs::create_dir_all(&user).unwrap();
    let prefs = user.join("Preferences.sublime-settings");
    let syntax = user.join("Plain Text.sublime-settings");
    std::fs::write(&prefs, r#"{"font_size":19}"#).unwrap();
    std::fs::write(&syntax, r#"{"tab_size":7}"#).unwrap();
    let mut a = app(Some(data.path()));
    assert_eq!(a.workspace.editor().preferences.font_size, 19.);
    assert_eq!(a.workspace.editor().settings.usize("tab_size", 4), 7);
    std::fs::write(&syntax, "{\n  \"tab_size\": ?\n}").unwrap();
    std::fs::write(&prefs, r#"{"font_size":21}"#).unwrap();
    a.workspace.request_resource_reload();
    let deadline = Instant::now() + Duration::from_secs(5);
    while a.workspace.editor().preferences.font_size != 21.
        || a.workspace.resource_reports.is_empty()
    {
        a.workspace.poll();
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(a.workspace.messages.is_empty());
    assert_eq!(a.workspace.editor().settings.usize("tab_size", 4), 7);
    a.publish_resource_reports();
    assert!(a.panels.message.contains("Plain Text.sublime-settings:2:"));
}

#[test]
fn r19_review_documented_macos_group_bindings_use_key_dispatch() {
    // The public macOS table documents groups 1..4 and column/grid layouts.
    // https://docs.sublimetext.io/reference/keyboard_shortcuts_osx.html#split-window
    let mut a = app(None);
    assert!(key(&mut a, "super+alt+5"));
    for group in 0..4 {
        assert!(key(&mut a, &format!("ctrl+{}", group + 1)));
        assert_eq!(a.workspace.workbench.active, group);
        let sheet = a.workspace.tabs[a.workspace.active].sheet;
        let target = (group + 1) % 4;
        assert!(key(&mut a, &format!("ctrl+shift+{}", target + 1)));
        assert_eq!(a.workspace.workbench.group_of(sheet), Some(target));
        validate(&a);
    }
}

#[test]
fn r19_review_regular_close_preserves_deleted_document_prompt() {
    let data = tempfile::tempdir().unwrap();
    let path = data.path().join("deleted.txt");
    std::fs::write(&path, "unchanged contents").unwrap();
    let mut a = app(Some(data.path()));
    named(&mut a, "open_file", json!({"file":path}));
    let sheet = a.workspace.tabs[a.workspace.active].sheet;
    std::fs::remove_file(&path).unwrap();
    a.workspace.tabs[a.workspace.active].deleted = true;
    a.close_decisions
        .push_back(tab_adapter::CloseDecision::Cancel);
    assert!(key(&mut a, "super+w"));
    assert!(ids(&a).contains(&sheet));
    assert!(a.close_decisions.is_empty());
    // Close Deleted's documented task contract asks only for edited contents.
    named(&mut a, "close_deleted_files", json!({}));
    assert!(!ids(&a).contains(&sheet));
    validate(&a);
}
