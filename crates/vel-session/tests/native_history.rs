use vel_buffer::{Buffer, Region, Selection, ViewId, ops};
use vel_session::History;

fn next(seed: &mut u64) -> u64 {
    *seed = seed
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    *seed
}
fn act(buffer: &mut Buffer, views: &[ViewId; 2], value: u64) -> bool {
    let id = views[(value >> 32) as usize % 2];
    match value % 10 {
        0..=2 => {
            ops::insert(
                buffer,
                id,
                ["a", "你好", "🦀", " ", "\n"][(value >> 16) as usize % 5],
            );
            true
        }
        3 => {
            ops::paste(buffer, id, "é!");
            true
        }
        4 => {
            let at = buffer
                .text()
                .floor_char_boundary((value >> 8) as usize % (buffer.text().len_bytes() + 1));
            buffer.set_selection(id, Selection::from_regions([Region::caret(at)]));
            true
        }
        5 => buffer.undo(id),
        6 => buffer.redo(id),
        7 => buffer.soft_undo(id),
        8 => buffer.soft_redo(id),
        _ => {
            buffer.mark_saved();
            true
        }
    }
}
fn same(a: &Buffer, av: &[ViewId; 2], b: &Buffer, bv: &[ViewId; 2]) {
    assert_eq!(a.text(), b.text());
    assert_eq!(a.is_dirty(), b.is_dirty());
    assert_eq!(a.history_limits().entries, b.history_limits().entries);
    assert_eq!(a.history_limits().bytes, b.history_limits().bytes);
    for i in 0..2 {
        assert_eq!(a.view(av[i]).sel(), b.view(bv[i]).sel());
    }
}
#[test]
fn randomized_quit_restore_two_views_preserves_all_future_history_behavior() {
    for run in 1..=128 {
        let mut seed = run;
        let mut live = Buffer::new("α\nhello\n");
        let views = [live.new_view(), live.new_view()];
        for _ in 0..200 {
            act(&mut live, &views, next(&mut seed));
        }
        let bytes = serde_json::to_vec(&History::capture(&live)).unwrap();
        let decoded: History = serde_json::from_slice(&bytes).unwrap();
        let (mut restored, map) = decoded.restore_views(true, live.is_dirty()).unwrap();
        let mapped = [map[&views[0]], map[&views[1]]];
        same(&live, &views, &restored, &mapped);
        for _ in 0..200 {
            let value = next(&mut seed);
            assert_eq!(
                act(&mut live, &views, value),
                act(&mut restored, &mapped, value),
                "seed {run}"
            );
            same(&live, &views, &restored, &mapped);
        }
    }
}
#[test]
fn typed_group_continues_after_restore_and_saved_point_survives() {
    let mut live = Buffer::new("");
    let id = live.new_view();
    ops::insert(&mut live, id, "h");
    ops::insert(&mut live, id, "e");
    let decoded: History =
        serde_json::from_slice(&serde_json::to_vec(&History::capture(&live)).unwrap()).unwrap();
    let (mut restored, map) = decoded.restore_views(true, true).unwrap();
    let view = map[&id];
    for s in ["l", "l", "o", " "] {
        ops::insert(&mut live, id, s);
        ops::insert(&mut restored, view, s);
    }
    assert!(restored.undo(view));
    assert!(live.undo(id));
    assert_eq!(live.text(), restored.text());
    assert!(!restored.is_dirty());
    assert_eq!(restored.is_dirty(), live.is_dirty());
}
#[test]
fn mismatch_discards_soft_and_text_history_and_malformed_native_data_never_panics() {
    let mut buffer = Buffer::new("hello");
    let id = buffer.new_view();
    ops::insert(&mut buffer, id, "你好");
    let history = History::capture(&buffer);
    let (mut restored, map) = history.restore_views(false, true).unwrap();
    assert!(!restored.undo(map[&id]));
    assert!(!restored.soft_undo(map[&id]));
    assert!(restored.is_dirty());
    let mut wire = serde_json::to_value(&history).unwrap();
    wire["snapshot"]["data"] = serde_json::json!("bad!");
    let decoded: History = serde_json::from_value(wire).unwrap();
    assert!(decoded.restore_views(true, true).is_err());
    let (text, _) = decoded.restore(false, true).unwrap();
    assert_eq!(text.text(), buffer.text());
}

#[test]
fn clean_file_history_is_lazy_and_keeps_soft_events_without_storing_current_text() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("clean.txt");
    std::fs::write(&path, "hello\n").unwrap();
    let mut live = Buffer::new("hello\n");
    let id = live.new_view();
    live.set_selection(id, Selection::from_regions([Region::caret(2)]));
    live.set_selection(id, Selection::from_regions([Region::caret(4)]));
    let history = History::capture_saved(&live, path.clone(), "UTF-8".into());
    let wire = serde_json::to_value(&history).unwrap();
    assert!(wire.get("text").is_none());
    let decoded: History = serde_json::from_value(wire).unwrap();
    let (mut restored, map) = decoded.restore_views(true, false).unwrap();
    assert_eq!(live.soft_undo(id), restored.soft_undo(map[&id]));
    assert_eq!(live.view(id).sel(), restored.view(map[&id]).sel());
    std::fs::remove_file(path).unwrap();
    assert!(decoded.restore_views(true, false).is_err());
}
#[test]
fn old_command_journal_profiles_upgrade_to_native_history() {
    let view = vel_session::ViewState::default();
    let wire = serde_json::json!({"base":"a","steps":[{"start":1,"end":1,"text":"你好"}],"cursor":1,"saved":0,"views":[view,view],"before":[view,view]});
    let history: History = serde_json::from_value(wire).unwrap();
    let (mut restored, id) = history.restore(true, true).unwrap();
    assert_eq!(restored.text(), "a你好");
    assert!(restored.undo(id));
    assert_eq!(restored.text(), "a");
    assert!(!restored.is_dirty());
    assert!(
        serde_json::to_value(history)
            .unwrap()
            .get("snapshot")
            .is_some()
    );
}
#[test]
fn closed_historical_edit_owners_remain_undoable_without_reopening_views() {
    let mut live = Buffer::new("");
    let open = live.new_view();
    let closed = live.new_view();
    ops::paste(&mut live, closed, "closed owner");
    live.close_view(closed);
    let wire = serde_json::to_vec(&History::capture(&live)).unwrap();
    let decoded: History = serde_json::from_slice(&wire).unwrap();
    let (mut restored, map) = decoded.restore_views(true, true).unwrap();
    assert_eq!(map.len(), 1);
    assert!(!map.contains_key(&closed));
    assert_eq!(live.undo(open), restored.undo(map[&open]));
    assert_eq!(live.text(), restored.text());
    assert_eq!(live.redo(open), restored.redo(map[&open]));
    assert_eq!(live.text(), restored.text());
}

#[test]
fn legacy_unsaved_cursor_zero_restores_dirty_without_a_synthetic_edit() {
    let view = vel_session::ViewState::default();
    let wire =
        serde_json::json!({"base":"hello", "steps":[], "cursor":0, "saved":null, "views":[view]});
    let history: History = serde_json::from_value(wire).unwrap();
    for keep in [false, true] {
        let (mut b, id) = history.restore(keep, true).unwrap();
        assert_eq!(b.text(), "hello");
        assert!(b.is_dirty(), "keep_history={keep}");
        assert!(!b.undo(id));
    }
}

#[test]
fn legacy_noop_steps_do_not_shift_the_undo_cursor_or_saved_revision() {
    for cursor in [1, 2] {
        for saved in [None, Some(0), Some(1), Some(2), Some(3)] {
            let view = vel_session::ViewState::default();
            let wire = serde_json::json!({"base":"", "steps":[
                {"start":0,"end":0,"text":"a"},
                {"start":1,"end":1,"text":""},
                {"start":1,"end":1,"text":"b"}
            ],"cursor":cursor,"saved":saved,"views":[view,view,view,view]});
            let history: History = serde_json::from_value(wire).unwrap();
            let (mut b, id) = history.restore(true, true).unwrap();
            assert_eq!(b.text(), "a", "cursor={cursor}, saved={saved:?}");
            assert_eq!(b.is_dirty(), !matches!(saved, Some(1 | 2)));
            assert!(b.undo(id));
            assert_eq!(b.text(), "");
            assert_eq!(b.is_dirty(), saved != Some(0));
            assert!(!b.undo(id));
            assert!(b.redo(id));
            assert_eq!(b.text(), "a");
            assert!(b.redo(id));
            assert_eq!(b.text(), "ab");
            assert_eq!(b.is_dirty(), saved != Some(3));
            assert!(!b.redo(id));
        }
    }
}
