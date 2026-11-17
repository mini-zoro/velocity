use super::*;
use vel_buffer::MonospaceLayout;
#[test]
fn passthrough_alt_keys_leave_pending_commands_and_recorded_macros_unchanged() {
    let mut b = Buffer::new("abcdefghi");
    let id = b.new_view();
    let mut vi = Vintage::new(Settings {
        vintage_start_in_command_mode: true,
        ..Default::default()
    });
    let layout = MonospaceLayout::default();
    for c in ['q', 'a', '2'] {
        vi.handle_key(c, &mut b, id, &layout);
    }
    let pending = vi.command_keys.clone();
    let recorded = vi.macros[&'a'].clone();
    assert_eq!(
        vi.handle_key(Key::Alt('x'), &mut b, id, &layout),
        Outcome::PassThrough
    );
    assert_eq!(vi.command_keys, pending);
    assert_eq!(vi.macros[&'a'], recorded);
    for c in ['x', 'q'] {
        vi.handle_key(c, &mut b, id, &layout);
    }
    assert_eq!(b.text().to_string(), "cdefghi");
    assert!(!vi.last_change.iter().any(|k| matches!(k, Key::Alt(_))));
    assert!(!vi.macros[&'a'].iter().any(|k| matches!(k, Key::Alt(_))));
    vi.handle_key('u', &mut b, id, &layout);
    vi.handle_key('.', &mut b, id, &layout);
    assert_eq!(b.text().to_string(), "cdefghi");
    for c in ['@', 'a'] {
        vi.handle_key(c, &mut b, id, &layout);
    }
    assert_eq!(b.text().to_string(), "efghi");
}
#[test]
fn history_clamps_stale_saved_endpoints_to_valid_scalar_boundaries() {
    let mut b = Buffer::new("猫abc");
    let id = b.new_view();
    let mut vi = Vintage::default();
    let layout = MonospaceLayout::default();
    vi.handle_key('X', &mut b, id, &layout);
    vi.handle_key(Key::Escape, &mut b, id, &layout);
    vi.undo.last_mut().unwrap().before = Selection::from_regions([
        Region {
            xpos: Some(13.),
            ..Region::caret(2)
        },
        Region::caret(999),
    ]);
    vi.handle_key('u', &mut b, id, &layout);
    assert_eq!(b.text().to_string(), "猫abc");
    for r in b.view(id).sel().iter() {
        assert!(b.text().is_char_boundary(r.a));
        assert!(r.end() <= b.text().len_bytes());
    }
}
#[test]
fn aggregate_host_insert_maps_preserve_unicode_payloads_and_dot_without_a_document_diff() {
    let mut b = Buffer::new(format!("ab{}cd", "unchanged ".repeat(10000)));
    let id = b.new_view();
    b.set_selection(id, Selection::from_regions([Region::caret(2)]));
    let original = b.text().clone();
    let mut vi = Vintage::default();
    vi.use_host_insert();
    vi.prepare_host_command("insert", &mut b, id);
    vel_buffer::ops::insert(&mut b, id, "猫");
    b.break_undo_group();
    vel_buffer::ops::insert(&mut b, id, "Z");
    vi.handle_key(Key::Escape, &mut b, id, &MonospaceLayout::default());
    let edited = b.text().to_string();
    assert!(edited.starts_with("ab猫Zunchanged"));
    vi.handle_key('u', &mut b, id, &MonospaceLayout::default());
    assert_eq!(b.text(), &original);
    vi.handle_key('.', &mut b, id, &MonospaceLayout::default());
    assert_eq!(b.text().to_string(), edited);
}

#[test]
fn host_completion_tracks_distinct_unicode_payloads_at_each_caret_for_undo_and_dot() {
    for replace_prefix in [false, true] {
        let mut b = Buffer::new("aa\nbb\ncc\n");
        let id = b.new_view();
        let old = Selection::from_regions([1, 4, 7].map(Region::caret));
        b.set_selection(id, old.clone());
        let mut vi = Vintage::default();
        vi.use_host_insert();
        vi.prepare_host_command("commit_completion", &mut b, id);
        let edits: Vec<_> = [1, 4, 7]
            .into_iter()
            .zip(["猫", "X", "Ω"])
            .map(|(at, text)| Edit {
                range: at - usize::from(replace_prefix)..at,
                text,
            })
            .collect();
        let map = vel_buffer::ChangeSet::new(
            edits
                .iter()
                .map(|e| vel_buffer::Change {
                    old_range: e.range.clone(),
                    new_len: e.text.len(),
                })
                .collect(),
        );
        b.transact(id, EditKind::Other, &edits, map.transform_selection(&old));
        let layout = MonospaceLayout::default();
        vi.handle_key(Key::Escape, &mut b, id, &layout);
        let expected = if replace_prefix {
            "猫a\nXb\nΩc\n"
        } else {
            "a猫a\nbXb\ncΩc\n"
        };
        assert_eq!(b.text().to_string(), expected);
        vi.handle_key('u', &mut b, id, &layout);
        assert_eq!(b.text().to_string(), "aa\nbb\ncc\n");
        vi.handle_key('.', &mut b, id, &layout);
        assert_eq!(b.text().to_string(), expected);
    }
}
