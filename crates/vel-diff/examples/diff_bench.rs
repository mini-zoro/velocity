//! Release 100k-line initial and local incremental diff timing.
use std::{hint::black_box, time::Instant};
use vel_buffer::{Change, ChangeId, ChangeSet};
use vel_diff::DocumentDiff;
use vel_text::Text;
fn main() {
    let original = (0..100_000)
        .map(|i| format!("line {i:06}: the quick brown fox jumps over a lazy dog\n"))
        .collect::<String>();
    let reference = Text::from(original.as_str());
    let mut text = reference.clone();
    for i in (0..100_000).step_by(997) {
        text.insert(text.line_to_byte(i), "changed ");
    }
    let start = Instant::now();
    let mut diff = black_box(DocumentDiff::new(reference, text.clone(), ChangeId(0)));
    let initial_ns = start.elapsed().as_nanos();
    let mut samples = Vec::new();
    for version in 1..=100 {
        let at = text.line_to_byte(50_000) + 15;
        let value = if version % 2 == 0 { "a" } else { "b" };
        text.replace(at..at + 1, value);
        let cs = ChangeSet::new(vec![Change {
            old_range: at..at + 1,
            new_len: 1,
        }]);
        let start = Instant::now();
        let stats = black_box(
            diff.apply_change(text.clone(), ChangeId(version), &cs)
                .unwrap(),
        );
        samples.push(start.elapsed().as_nanos());
        assert!(stats.new_lines < 10);
    }
    println!(
        "{{\"initial_ns\":{initial_ns},\"edit_ns\":{samples:?},\"lines\":100000,\"bytes\":{}}}",
        text.len_bytes()
    );
}
