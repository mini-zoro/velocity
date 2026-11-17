//! Explicit paired stress regressions; keep independent of the parallel workspace gate.
use super::*;
use std::{
    hint::black_box,
    time::{Duration, Instant},
};
use vel_buffer::{Region, Selection};
use vel_config::{json, settings::Layer};
use vel_text::Text;
fn median(mut xs: Vec<Duration>) -> Duration {
    xs.sort_unstable();
    xs[xs.len() / 2]
}
#[test]
#[ignore = "paired stress timing; run explicitly without concurrent compilation"]
fn five_thousand_folds_and_carets_have_a_small_constant_edit_cost() {
    let mut times = [Vec::new(), Vec::new()];
    for pair in 0..7 {
        for folded in if pair % 2 == 0 {
            [false, true]
        } else {
            [true, false]
        } {
            let mut e = Editor::new(Text::from("ab hidden z\n".repeat(5000)));
            e.set_selection(Selection::from_regions(
                (0..5000).map(|n| Region::caret(n * 12 + 2)),
            ));
            if folded {
                e.set_folded(
                    (0..5000)
                        .map(|n| Region::new(n * 12 + 3, n * 12 + 9))
                        .collect(),
                );
            }
            let at = Instant::now();
            e.run("left_delete", &json!({})).unwrap();
            e.run("insert", &json!({"characters":"X"})).unwrap();
            times[usize::from(folded)].push(at.elapsed());
            assert_eq!(e.text.to_string(), "aX hidden z\n".repeat(5000));
            assert_eq!(e.folded().len(), if folded { 5000 } else { 0 });
        }
    }
    let [plain, folded] = times.map(median);
    println!(
        "R16_STRESS folds plain_ms={} folded_ms={} ratio={}",
        plain.as_secs_f64() * 1000.,
        folded.as_secs_f64() * 1000.,
        folded.as_secs_f64() / plain.as_secs_f64()
    );
    assert!(folded <= plain * 3 + Duration::from_millis(2));
}
#[test]
#[ignore = "paired stress timing; run explicitly without concurrent compilation"]
fn ten_thousand_caret_vintage_insert_tracks_normal_editing_cost() {
    let mut times = [Vec::new(), Vec::new()];
    for pair in 0..7 {
        for vintage in if pair % 2 == 0 {
            [false, true]
        } else {
            [true, false]
        } {
            let mut e = Editor::new(Text::from("ab\n".repeat(10000)));
            e.set_selection(Selection::from_regions(
                (0..10000).map(|n| Region::caret(n * 3 + 1)),
            ));
            if vintage {
                e.settings.set(Layer::View, "vintage_enabled", json!(true));
                e.sync();
            }
            for n in 0..8 {
                let at = Instant::now();
                e.text_input(vel_platform::TextInput::Insert {
                    text: "X".into(),
                    replacement: None,
                });
                if n > 1 {
                    times[usize::from(vintage)].push(at.elapsed());
                }
            }
            assert_eq!(e.text.to_string(), "aXXXXXXXXb\n".repeat(10000));
        }
    }
    let [plain, vintage] = times.map(median);
    println!(
        "R16_STRESS vintage plain_ms={} vintage_ms={} ratio={}",
        plain.as_secs_f64() * 1000.,
        vintage.as_secs_f64() * 1000.,
        vintage.as_secs_f64() / plain.as_secs_f64()
    );
    assert!(
        vintage <= plain * 3,
        "10k-caret observation must stay within 3× ordinary editing"
    );
}
#[test]
#[ignore = "paired stress timing; run explicitly without concurrent compilation"]
fn two_hundred_thousand_bookmarks_do_not_scan_the_document_on_paint() {
    let mut e = Editor::new(Text::from("a\n".repeat(200000)));
    let mut v = View::new(
        Size {
            width: 600.,
            height: 320.,
        },
        8.,
        12.,
    );
    let mut times = [Vec::new(), Vec::new()];
    for marked in [false, true] {
        if marked {
            e.set_selection(Selection::from_regions(
                (0..200000).map(|n| Region::caret(n * 2)),
            ));
            e.run("toggle_bookmark", &json!({})).unwrap();
            e.set_caret(0, false);
        }
        for n in 0..110 {
            v.scroll_y = ((n * 97) % 199900) as f64 * v.line_height;
            black_box(v.scene(&e));
            let at = Instant::now();
            black_box(v.scene(&e));
            if n >= 10 {
                times[usize::from(marked)].push(at.elapsed());
            }
        }
    }
    let [plain, marked] = times.map(median);
    println!(
        "R16_STRESS bookmarks plain_ms={} marked_ms={} ratio={}",
        plain.as_secs_f64() * 1000.,
        marked.as_secs_f64() * 1000.,
        marked.as_secs_f64() / plain.as_secs_f64()
    );
    assert!(marked <= (plain * 3).max(Duration::from_micros(150)));
}
