//! Regression probes from the bdb5a7a verification, through the App entry points.
use super::*;
use tab_adapter::CloseDecision;

fn restore(data: &Path) -> App {
    let mut a = app(Some(data));
    let session = a.sessions.borrow().restored.clone();
    let reserved = a.sessions.borrow_mut().reserve_recovered();
    let window = &session.windows[0];
    a.workspace
        .restore_window(window, &session.documents)
        .unwrap();
    a.restore_ui(window);
    a.restore_session_id(reserved[0]);
    a.sync_active_view();
    a
}
fn save_checkpoint(a: &mut App) {
    a.checkpoint(false).unwrap();
    a.sessions.borrow().flush().unwrap();
}
fn cloned(data: &Path, original_visible: bool) -> App {
    let mut a = app(Some(data));
    named(&mut a, "select_all", json!({}));
    named(&mut a, "insert", json!({"characters":"hello"}));
    named(&mut a, "clone_file", json!({}));
    if original_visible {
        named(&mut a, "select_by_index", json!({"index":0}));
    }
    save_checkpoint(&mut a);
    drop(a);
    let a = restore(data);
    assert_eq!(
        a.workspace.tabs.iter().filter(|t| t.lazy.is_some()).count(),
        1
    );
    assert_eq!(a.workspace.active == 0, original_visible);
    let owner = &a.workspace.tabs[a.workspace.active];
    assert!(
        a.workspace
            .tabs
            .iter()
            .all(|t| t.document == owner.document)
    );
    a
}

#[test]
fn b19_restored_lazy_clone_close_preserves_edits_for_both_decisions_and_later_save() {
    // Adopted b19_restored_lazy_clone_* repros: the queued answer must *not* be
    // consumed while a sibling exists; saving the eventual last view retains edits.
    for original_visible in [false, true] {
        for save in [false, true] {
            let data = tempfile::tempdir().unwrap();
            let saved = data.path().join("saved.txt");
            let mut a = cloned(data.path(), original_visible);
            named(&mut a, "insert", json!({"characters":" world"}));
            a.close_decisions.push_back(if save {
                CloseDecision::Save(Some(saved.clone()))
            } else {
                CloseDecision::Discard
            });
            assert!(key(&mut a, "super+w"));
            assert_eq!(a.close_decisions.len(), 1, "closing a sibling prompted");
            assert_eq!(a.workspace.tabs.len(), 1);
            assert_eq!(a.workspace.editor().text.to_string(), "hello world");
            assert!(a.workspace.document_contents_dirty(&a.workspace.tabs[0]));
            named(&mut a, "insert", json!({"characters":"!"}));
            a.close_decisions.clear();
            a.close_decisions
                .push_back(CloseDecision::Save(Some(saved.clone())));
            assert!(key(&mut a, "super+w"));
            assert!(a.workspace.is_empty());
            assert_eq!(std::fs::read_to_string(saved).unwrap(), "hello world!");
            validate(&a);
        }
    }
}

#[test]
fn b19_restored_lazy_clone_save_as_attaches_to_live_buffer_and_history() {
    for original_visible in [false, true] {
        let data = tempfile::tempdir().unwrap();
        let path = data.path().join("saved.txt");
        let mut a = cloned(data.path(), original_visible);
        named(&mut a, "insert", json!({"characters":" world"}));
        let active = a.workspace.active;
        assert!(
            a.save_tab_with_path(None, active, true, Some(path.clone()))
                .unwrap()
        );
        assert_eq!(
            a.workspace.tabs.iter().filter(|t| t.lazy.is_some()).count(),
            1
        );
        assert!(key(&mut a, "super+w"));
        assert_eq!(a.workspace.editor().text.to_string(), "hello world");
        assert_eq!(
            a.workspace.tabs[a.workspace.active].path.as_ref(),
            Some(&path.canonicalize().unwrap())
        );
        named(&mut a, "insert", json!({"characters":"!"}));
        assert!(key(&mut a, "super+s"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "hello world!");
        save_checkpoint(&mut a);
        drop(a);
        let mut a = restore(data.path());
        assert_eq!(a.workspace.editor().text.to_string(), "hello world!");
        named(&mut a, "undo", json!({}));
        assert_eq!(a.workspace.editor().text.to_string(), "hello world");
        validate(&a);
    }
}

#[test]
fn b19_restored_lazy_clone_attaches_rebased_selection_and_folds() {
    let data = tempfile::tempdir().unwrap();
    let mut a = app(Some(data.path()));
    let text = "top\n  a\n  b\nbottom\n";
    named(&mut a, "select_all", json!({}));
    named(&mut a, "insert", json!({"characters":text}));
    a.workspace
        .editor_mut()
        .set_selection(vel_buffer::Selection::from_regions([
            vel_buffer::Region::new(4, 12),
        ]));
    let deadline = Instant::now() + crate::test_timing::HANG_GUARD;
    while a.workspace.editor().folded().is_empty() {
        named(&mut a, "fold", json!({}));
        a.workspace.poll();
        assert!(Instant::now() < deadline);
    }
    named(&mut a, "move_to", json!({"to":"eof"}));
    named(&mut a, "clone_file", json!({}));
    named(&mut a, "select_by_index", json!({"index":0}));
    named(&mut a, "move_to", json!({"to":"bof"}));
    save_checkpoint(&mut a);
    drop(a);
    let mut a = restore(data.path());
    assert!(a.workspace.tabs[1].lazy.is_some());
    named(&mut a, "insert", json!({"characters":"prefix\n"}));
    a.close_decisions.push_back(CloseDecision::Cancel);
    assert!(key(&mut a, "super+w"));
    assert_eq!(a.close_decisions.len(), 1);
    assert_eq!(
        a.workspace.editor().text.to_string(),
        format!("prefix\n{text}")
    );
    assert_eq!(
        a.workspace.editor().regions(),
        vec![vel_buffer::Region::caret(text.len() + 7)]
    );
    assert_eq!(
        a.workspace.editor().folded(),
        vec![vel_buffer::Region::new(11, 19)]
    );
    validate(&a);
}

#[test]
fn r19_verify_close_all_is_palette_only_and_stops_on_cancel() {
    let data = tempfile::tempdir().unwrap();
    let mut a = app(None);
    for n in 0..3 {
        named(&mut a, "new_file", json!({}));
        named(&mut a, "insert", json!({"characters":format!("dirty {n}")}));
    }
    let sheets = a.workspace.workbench.groups[0].sheets.clone();
    let file = a
        .workspace
        .menus
        .iter()
        .find(|m| m.caption == "File")
        .unwrap();
    assert!(
        !file
            .children
            .iter()
            .any(|m| m.command.as_deref() == Some("close_all"))
    );
    let close = a
        .workspace
        .palette
        .iter()
        .find(|item| item.invocation.command == "close_all")
        .unwrap();
    let command = close.invocation.command.clone();
    assert!(a.workspace.editor().registry.kind(&command).is_some());
    let saved = data.path().join("last.txt");
    a.close_decisions.extend([
        CloseDecision::Save(Some(saved.clone())),
        CloseDecision::Discard,
        CloseDecision::Cancel,
        CloseDecision::Discard,
    ]);
    named(&mut a, &command, json!({}));
    assert_eq!(ids(&a), sheets[..2].iter().copied().collect());
    assert_eq!(std::fs::read_to_string(saved).unwrap(), "dirty 2");
    assert_eq!(a.close_decisions.len(), 1);
    named(&mut a, &command, json!({}));
    assert!(a.workspace.is_empty());
    assert!(a.close_decisions.is_empty());
    validate(&a);
}

fn settled(a: &mut App) {
    a.workspace.wait_resources();
    a.publish_resource_reports();
    assert!(
        a.workspace.messages.is_empty(),
        "{:?}",
        a.workspace.messages
    );
}

#[test]
fn r19_verify_preference_writers_report_broken_file_nonmodally_once_per_revision() {
    for command in [
        "toggle_setting",
        "increase_font_size",
        "decrease_font_size",
        "reset_font_size",
    ] {
        let data = tempfile::tempdir().unwrap();
        let user = data.path().join("Packages/User");
        std::fs::create_dir_all(&user).unwrap();
        let path = user.join("Preferences.sublime-settings");
        std::fs::write(&path, "{\"font_size\":19}").unwrap();
        let mut a = app(Some(data.path()));
        settled(&mut a);
        let broken = "{\n  \"font_size\": 19\n  \"word_wrap\":true\n}";
        std::fs::write(&path, broken).unwrap();
        named(&mut a, command, json!({"setting":"save_all_on_build"}));
        settled(&mut a);
        assert!(
            a.panels.message.contains("Preferences.sublime-settings"),
            "{command}: {}",
            a.panels.message
        );
        assert!(
            a.panels.message.contains(".sublime-settings:3:"),
            "{}",
            a.panels.message
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), broken);
        a.panels.message.clear();
        named(&mut a, command, json!({"setting":"save_all_on_build"}));
        settled(&mut a);
        assert!(a.panels.message.is_empty(), "same revision reported again");
        std::fs::write(&path, "{\n\"font_size\":?\n}").unwrap();
        named(&mut a, command, json!({"setting":"save_all_on_build"}));
        settled(&mut a);
        assert!(a.panels.message.contains(".sublime-settings:2:"));
    }
}

#[test]
fn r19_verify_settings_errors_report_only_in_initiating_window_and_not_on_unrelated_reload() {
    let data = tempfile::tempdir().unwrap();
    let user = data.path().join("Packages/User");
    std::fs::create_dir_all(&user).unwrap();
    let path = user.join("Preferences.sublime-settings");
    std::fs::write(&path, "{\"font_size\":19}").unwrap();
    let mut a = app(Some(data.path()));
    let mut b = app(Some(data.path()));
    named(&mut b, "open_file", json!({"file":path}));
    named(&mut b, "select_all", json!({}));
    named(
        &mut b,
        "insert",
        json!({"characters":"{\n \"font_size\":?\n}"}),
    );
    assert!(key(&mut b, "super+s"));
    // The *other* window can finish its loader first. It must not consume the report.
    a.workspace.request_resource_reload();
    settled(&mut a);
    assert!(a.panels.message.is_empty(), "{}", a.panels.message);
    b.workspace.request_resource_reload();
    settled(&mut b);
    assert!(b.panels.message.contains("Preferences.sublime-settings"));
    assert!(b.panels.message.contains(".sublime-settings:2:"));
    assert_eq!(a.workspace.editor().preferences.font_size, 19.);
    assert_eq!(b.workspace.editor().preferences.font_size, 19.);
    a.panels.message.clear();
    b.panels.message.clear();
    std::fs::write(user.join("Plain Text.sublime-settings"), "{\"tab_size\":7}").unwrap();
    for window in [&mut a, &mut b] {
        window.workspace.request_resource_reload();
        settled(window);
        assert!(
            window.panels.message.is_empty(),
            "unchanged error: {}",
            window.panels.message
        );
        assert_eq!(window.workspace.editor().preferences.font_size, 19.);
    }
    drop(b);
    a.workspace.request_resource_reload();
    settled(&mut a);
    assert!(
        a.panels.message.is_empty(),
        "closing the reporter repeated the error"
    );
    let mut c = app(Some(data.path()));
    settled(&mut c);
    assert!(
        c.panels.message.is_empty(),
        "opening another window repeated the error"
    );
}
