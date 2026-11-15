use proptest::prelude::*;
use vel_buffer::{Change, ChangeId, ChangeSet};
use vel_diff::{DocumentDiff, HunkKind, InlineMode};
use vel_text::{Edit, Text};
fn assert_script(diff: &DocumentDiff) {
    let mut result = diff.text().clone();
    let plans = diff.revert_all();
    result.apply_edits(
        &plans
            .iter()
            .map(|p| Edit {
                range: p.range.clone(),
                text: &p.text,
            })
            .collect::<Vec<_>>(),
    );
    assert_eq!(&result, diff.reference());
    let mut old = 0;
    let mut new = 0;
    let count = |t: &Text| {
        if t.is_empty() {
            0
        } else {
            t.len_lines() - usize::from(t.byte_at(t.len_bytes() - 1) == b'\n')
        }
    };
    let byte = |t: &Text, l| {
        if l >= count(t) {
            t.len_bytes()
        } else {
            t.line_to_byte(l)
        }
    };
    for h in diff.hunks() {
        assert!(h.old.start >= old && h.new.start >= new);
        assert_eq!(
            diff.reference()
                .slice(byte(diff.reference(), old)..byte(diff.reference(), h.old.start)),
            diff.text()
                .slice(byte(diff.text(), new)..byte(diff.text(), h.new.start))
        );
        old = h.old.end;
        new = h.new.end;
    }
    assert_eq!(
        diff.reference()
            .slice(byte(diff.reference(), old)..diff.reference().len_bytes()),
        diff.text()
            .slice(byte(diff.text(), new)..diff.text().len_bytes())
    );
}
proptest! {
    #![proptest_config(ProptestConfig::default())]
    #[test]
    fn edits_keep_an_exact_revert_script(initial in "[ab中🦀\n\r]{0,180}", edits in prop::collection::vec((any::<usize>(),any::<usize>(),"[ab世\n\r]{0,15}"),1..80)) {
        let mut text = Text::from(initial.as_str()); let mut diff = DocumentDiff::new(text.clone(), text.clone(), ChangeId(0));
        for (i,(a,b,insert)) in edits.into_iter().enumerate() {
            let a=text.floor_char_boundary(a%(text.len_bytes()+1)); let b=text.floor_char_boundary(b%(text.len_bytes()+1)); let range=a.min(b)..a.max(b);
            text.apply_edits(&[Edit { range: range.clone(), text: &insert }]);
            diff.apply_change(text.clone(), ChangeId(i as u64+1), &ChangeSet::new(vec![Change { old_range: range, new_len: insert.len() }])).unwrap();
            assert_script(&diff);
        }
    }
}
#[test]
fn local_edits_do_not_scan_tail_and_disjoint_windows_stay_separate() {
    let original = (0..100_000)
        .map(|i| format!("line {i}\n"))
        .collect::<String>();
    let mut text = Text::from(original.as_str());
    let mut diff = DocumentDiff::new(text.clone(), text.clone(), ChangeId(0));
    let a = text.line_to_byte(40_000) + 2;
    let b = text.line_to_byte(80_000) + 3;
    let changes = ChangeSet::new(vec![
        Change {
            old_range: a..a + 1,
            new_len: 1,
        },
        Change {
            old_range: b..b + 1,
            new_len: 1,
        },
    ]);
    text.apply_edits(&[
        Edit {
            range: a..a + 1,
            text: "X",
        },
        Edit {
            range: b..b + 1,
            text: "Y",
        },
    ]);
    let stats = diff
        .apply_change(text.clone(), ChangeId(1), &changes)
        .unwrap();
    assert_eq!(stats.windows, 2);
    assert!(stats.new_lines <= 12);
    assert_eq!(diff.hunks().len(), 2);
    assert_script(&diff);
    assert_eq!(diff.next_modification(0, false), Some(40_000));
    assert_eq!(diff.prev_modification(99_999, false), Some(80_000));
}
#[test]
fn lf_cr_unicode_empty_eof_revert_and_inline() {
    for (a, b) in [
        ("", "世\n"),
        ("a\n", ""),
        ("a", "a\n"),
        ("a\r\n世", "a\n🦀"),
        ("a\nb\n", "a\n"),
    ] {
        let diff = DocumentDiff::new(a, Text::from(b), ChangeId(0));
        assert_script(&diff);
        for h in diff.hunks() {
            let inline = diff.inline_diff(h, InlineMode::Character).unwrap();
            for c in inline {
                assert!(diff.reference().is_char_boundary(c.old.start));
                assert!(diff.text().is_char_boundary(c.new.end));
            }
        }
    }
    let diff = DocumentDiff::new("a\nb\n", Text::from("a\n"), ChangeId(0));
    assert_eq!(diff.hunks()[0].kind(), HunkKind::Deleted);
}
#[test]
fn invalid_changes_are_atomic() {
    let mut diff = DocumentDiff::new("a", Text::from("a"), ChangeId(1));
    assert!(
        diff.apply_change(Text::from("b"), ChangeId(1), &ChangeSet::default())
            .is_err()
    );
    assert!(
        diff.apply_change(Text::from("bb"), ChangeId(2), &ChangeSet::default())
            .is_err()
    );
    assert_eq!(diff.text().to_string(), "a");
}
