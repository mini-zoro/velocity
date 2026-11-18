use proptest::prelude::*;
use std::collections::BTreeMap;
use vel_buffer::{
    Bias, Buffer, Edit, EditKind, HistoryError, HistorySnapshot, Region, Selection, ViewId, ops,
};

fn remap(b: &Buffer) -> BTreeMap<ViewId, ViewId> {
    b.views().map(|v| (v, ViewId::fresh())).collect()
}
fn restore(b: &Buffer) -> (Buffer, BTreeMap<ViewId, ViewId>) {
    let snapshot = b.export_history();
    let json = serde_json::to_vec(&snapshot).unwrap();
    let decoded: HistorySnapshot = serde_json::from_slice(&json).unwrap();
    let map = remap(b);
    (
        Buffer::restore_history(b.text().clone(), decoded, &map).unwrap(),
        map,
    )
}
fn same(a: &Buffer, b: &Buffer, map: &BTreeMap<ViewId, ViewId>) {
    assert_eq!(a.text(), b.text());
    assert_eq!(a.is_dirty(), b.is_dirty());
    assert_eq!(a.history_entries(), b.history_entries());
    for (&old, &new) in map {
        assert_eq!(a.view(old).sel(), b.view(new).sel());
    }
}
fn command(b: &mut Buffer, view: ViewId, action: u8, position: usize) -> Option<bool> {
    match action % 10 {
        0 => Some(b.undo(view)),
        1 => Some(b.redo(view)),
        2 => Some(b.soft_undo(view)),
        3 => Some(b.soft_redo(view)),
        4 => {
            let p = b
                .text()
                .floor_char_boundary(position % (b.text().len_bytes() + 1));
            b.set_selection(view, Selection::from(Region::caret(p)));
            None
        }
        5 => {
            ops::insert(b, view, "世🦀\n");
            None
        }
        6 => {
            ops::insert(b, view, "x");
            None
        }
        7 => {
            let a = b
                .text()
                .floor_char_boundary(position % (b.text().len_bytes() + 1));
            let z = b.text().next_grapheme_boundary(a);
            b.transact(
                view,
                EditKind::Delete,
                &[Edit {
                    range: a..z,
                    text: "",
                }],
                Selection::from(Region::caret(a)),
            );
            None
        }
        8 => {
            b.mark_saved();
            None
        }
        _ => {
            b.break_undo_group();
            None
        }
    }
}
proptest! {
    #![proptest_config(ProptestConfig::default())]
    #[test]
    fn arbitrary_history_continues_losslessly(actions in prop::collection::vec((any::<u8>(),any::<usize>(), any::<bool>()),1..150), future in prop::collection::vec((any::<u8>(),any::<usize>(),any::<bool>()),1..150)) {
        let mut original = Buffer::new("hello 世界\nline\r\n🦀");
        let a = original.new_view(); let c = original.new_view();
        for &(action, pos, second) in &actions { command(&mut original, if second { c } else { a }, action, pos); }
        let (mut restored, map) = restore(&original); same(&original, &restored, &map);
        for (action, pos, second) in future {
            let old = if second { c } else { a }; let new = map[&old];
            assert_eq!(command(&mut original, old, action, pos), command(&mut restored, new, action, pos));
            same(&original, &restored, &map);
        }
        for _ in 0..160 { assert_eq!(original.undo(a), restored.undo(map[&a])); same(&original, &restored, &map); }
        for _ in 0..160 { assert_eq!(original.redo(a), restored.redo(map[&a])); same(&original, &restored, &map); }
    }
}
#[test]
fn pruned_history_closed_owners_maps_and_capture_are_owned() {
    let mut b = Buffer::new("abc\n");
    let a = b.new_view();
    let c = b.new_view();
    b.set_history_limits(vel_buffer::HistoryLimits {
        entries: 8,
        changes: 11,
        ..Default::default()
    });
    for _ in 0..20 {
        ops::insert(&mut b, a, "x");
        b.break_undo_group();
    }
    ops::insert(&mut b, c, "世");
    b.close_view(c);
    b.add_regions(a, "note", vec![Region::new(0, 1)], false);
    let (mut restored, map) = restore(&b);
    same(&b, &restored, &map);
    assert_eq!(
        restored.view(map[&a]).get_regions("note").unwrap().regions,
        vec![Region::new(0, 1)]
    );
    let old = b.change_id();
    ops::insert(&mut b, a, "!");
    let changes = b.changes_since(old).unwrap();
    assert_eq!(changes.len(), 1);
    let snapshot = b.export_history();
    let captured = b.text().clone();
    let map2 = remap(&b);
    ops::insert(&mut b, a, "later");
    let captured_buffer = Buffer::restore_history(captured.clone(), snapshot, &map2).unwrap();
    assert_eq!(captured_buffer.text(), &captured);
    for _ in 0..8 {
        assert!(restored.undo(map[&a]));
    }
}
#[test]
fn mismatch_version_mapping_and_corrupt_data_fail_cleanly() {
    let mut b = Buffer::new("世界");
    let v = b.new_view();
    ops::insert(&mut b, v, "x");
    let map = remap(&b);
    assert!(matches!(
        Buffer::restore_history("nope", b.export_history(), &map),
        Err(HistoryError::TextMismatch)
    ));
    assert!(matches!(
        Buffer::restore_history(b.text().clone(), b.export_history(), &BTreeMap::new()),
        Err(HistoryError::ViewMap)
    ));
    let mut value = serde_json::to_value(b.export_history()).unwrap();
    value["version"] = 999.into();
    assert!(matches!(
        Buffer::restore_history(
            b.text().clone(),
            serde_json::from_value(value).unwrap(),
            &map
        ),
        Err(HistoryError::Version(999))
    ));
    let mut value = serde_json::to_value(b.export_history()).unwrap();
    value["data"] = "corrupt".into();
    assert!(
        Buffer::restore_history(
            b.text().clone(),
            serde_json::from_value(value).unwrap(),
            &map
        )
        .is_err()
    );
}
#[test]
fn relative_change_maps_survive_restart() {
    let mut b = Buffer::new("abc");
    let v = b.new_view();
    let origin = b.change_id();
    ops::insert(&mut b, v, "世");
    let before = b.transform_point_from(2, origin, Bias::Right);
    let (restored, _) = restore(&b);
    let from = vel_buffer::ChangeId(restored.change_id().0 - (b.change_id().0 - origin.0));
    assert_eq!(restored.transform_point_from(2, from, Bias::Right), before);
}

#[test]
fn ten_thousand_typing_steps_are_compact_and_roundtrip() {
    let mut b = Buffer::new("");
    let v = b.new_view();
    for _ in 0..10_000 {
        ops::insert(&mut b, v, "x");
    }
    let json = serde_json::to_vec(&b.export_history()).unwrap();
    assert!(
        json.len() <= 20_000,
        "{} serialized bytes for 10,000 raw edit bytes",
        json.len()
    );
    let map = remap(&b);
    let snapshot = serde_json::from_slice(&json).unwrap();
    let mut restored = Buffer::restore_history(b.text().clone(), snapshot, &map).unwrap();
    assert!(restored.undo(map[&v]));
    assert!(restored.text().is_empty());
    assert!(restored.redo(map[&v]));
    same(&b, &restored, &map);
}

#[test]
fn directed_primary_goals_and_multi_caret_steps_survive() {
    let mut b = Buffer::new("abc 世界\nlast");
    let v = b.new_view();
    let selection = Selection::from_regions([
        Region {
            a: 11,
            b: 11,
            xpos: Some(12.0),
        },
        Region {
            a: 3,
            b: 0,
            xpos: Some(4.5),
        },
    ]);
    assert_eq!(selection.primary_index(), 0);
    b.set_selection(v, selection);
    ops::insert(&mut b, v, "🦀");
    b.mark_saved();
    b.set_selection(v, Selection::from(Region::caret(0)));
    assert!(b.soft_undo(v));
    let (mut restored, map) = restore(&b);
    same(&b, &restored, &map);
    assert_eq!(b.undo(v), restored.undo(map[&v]));
    same(&b, &restored, &map);
    assert_eq!(b.redo(v), restored.redo(map[&v]));
    same(&b, &restored, &map);
}
#[test]
fn consecutive_deletions_in_one_direction_are_one_undo_step() {
    let mut b = Buffer::new("hello world");
    let v = b.new_view();
    b.set_selection(v, Selection::from(Region::caret(5)));
    for _ in 0..3 {
        ops::delete_left(&mut b, v, ops::DeleteBy::Char);
    }
    assert_eq!(b.text().to_string(), "he world");
    // Switching direction starts a new step.
    ops::delete_right(&mut b, v, ops::DeleteBy::Char);
    assert_eq!(b.text().to_string(), "heworld");
    assert!(b.undo(v));
    assert_eq!(b.text().to_string(), "he world");
    assert!(b.undo(v));
    assert_eq!(b.text().to_string(), "hello world");
    // Moving the caret in between breaks the run.
    b.set_selection(v, Selection::from(Region::caret(5)));
    ops::delete_left(&mut b, v, ops::DeleteBy::Char);
    b.set_selection(v, Selection::from(Region::caret(2)));
    ops::delete_left(&mut b, v, ops::DeleteBy::Char);
    assert_eq!(b.text().to_string(), "hll world");
    assert!(b.undo(v));
    assert_eq!(b.text().to_string(), "hell world");
}

#[test]
fn reducing_history_limits_bounds_selection_soft_redo() {
    let mut buffer = Buffer::new("x".repeat(1024).as_str());
    let view = buffer.new_view();
    buffer.set_history_limits(vel_buffer::HistoryLimits {
        selection_entries: 1024,
        ..buffer.history_limits()
    });
    for at in 1..=600 {
        buffer.set_selection(view, Selection::from(Region::caret(at)));
    }
    for _ in 0..600 {
        assert!(buffer.soft_undo(view));
    }
    buffer.set_history_limits(vel_buffer::HistoryLimits {
        selection_entries: 256,
        ..buffer.history_limits()
    });
    for _ in 0..256 {
        assert!(buffer.soft_redo(view));
    }
    assert!(!buffer.soft_redo(view));
}
