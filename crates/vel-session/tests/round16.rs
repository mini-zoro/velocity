use vel_buffer::{Buffer, Region, Selection, ops};
use vel_session::{History, ViewState};

#[test]
fn bookmark_and_mark_round_trip_with_and_without_native_history() {
    let mut buffer = Buffer::new("aa 猫\nbb\ncc");
    let id = buffer.new_view();
    buffer.set_selection(
        id,
        Selection::from_regions([Region::new(6, 3), Region::caret(10)]),
    );
    ops::toggle_bookmark(&mut buffer, id);
    ops::set_mark(&mut buffer, id);
    buffer.set_selection(id, Selection::from(Region::caret(0)));
    ops::insert(&mut buffer, id, "xx\n");
    let state = ViewState::capture(&buffer, id, (12., 34.));
    let wire = serde_json::to_vec(&state).unwrap();
    let state: ViewState = serde_json::from_slice(&wire).unwrap();
    state.validate().unwrap();
    let history: History =
        serde_json::from_slice(&serde_json::to_vec(&History::capture(&buffer)).unwrap()).unwrap();
    for keep in [true, false] {
        let (mut restored, view) = history.restore(keep, true).unwrap();
        state.apply(&mut restored, view);
        assert!(restored.is_dirty());
        for key in [ops::BOOKMARKS, ops::MARK, "velocity.mark.primary"] {
            assert_eq!(
                restored.view(view).get_regions(key).unwrap().regions,
                buffer.view(id).get_regions(key).unwrap().regions,
                "{key}, history={keep}"
            );
        }
        ops::select_all_bookmarks(&mut restored, view);
        assert_eq!(
            restored.view(view).sel().iter().collect::<Vec<_>>(),
            [Region::new(9, 6), Region::caret(13)]
        );
        restored.set_selection(view, Selection::from(Region::caret(0)));
        assert!(ops::swap_with_mark(&mut restored, view));
        assert_eq!(restored.view(view).sel().primary(), Some(Region::caret(13)));
    }
}

#[test]
fn legacy_view_state_and_clipped_named_regions_restore() {
    let mut state: ViewState = serde_json::from_value(serde_json::json!({
        "selections":[{"a":0,"b":0}],"primary":0,"scroll":[0,0],"folds":[]
    }))
    .unwrap();
    state.validate().unwrap();
    let mut b = Buffer::new("猫");
    let id = b.new_view();
    state.named_regions.insert(
        ops::BOOKMARKS.into(),
        vec![vel_session::Position {
            a: 1,
            b: 99,
            xpos: None,
        }],
    );
    state.apply(&mut b, id);
    assert_eq!(
        b.view(id).get_regions(ops::BOOKMARKS).unwrap().regions,
        [Region::new(0, 3)]
    );
    state.named_regions.get_mut(ops::BOOKMARKS).unwrap()[0].xpos = Some(f64::NAN);
    assert!(state.validate().is_err());
}
