use proptest::prelude::*;
use vel_buffer::ChangeId;
use vel_highlight::{ScopeSnapshot, ScopeStackId, ScopeStacks, ScopedLine};
use vel_text::Text;

proptest! {
    #[test]
    fn counted_completeness_matches_scan_for_all_exactness_patterns(
        flags in prop::collection::vec(any::<bool>(), 0..10_000),
        split in any::<usize>(),
        pattern in 0u8..4,
    ) {
        // Construction accumulates inexactness in the same pass as the records.
        // Rebuild arbitrary prefixes/suffixes, as viewport requests and edits do.
        let text = Text::from("x\n".repeat(flags.len()));
        let split = split % (flags.len() + 1);
        let mut flags = flags;
        match pattern {
            0 => flags.fill(true),
            1 => flags.fill(false),
            2 => {
                flags.fill(true);
                if let Some(line) = flags.get_mut(split) {
                    *line = false;
                }
            }
            _ => {},
        }
        for range in [0..flags.len(), 0..split, split..flags.len(), split..split] {
            let snapshot = ScopeSnapshot {
                version: ChangeId(17), text: text.clone(), range: range.clone(),
                stacks: ScopeStacks::default(),
                lines: range.map(|line| if flags[line] {
                    ScopedLine::empty(ScopeStackId(0))
                } else {
                    ScopedLine::default()
                }).collect(),
            };
            prop_assert_eq!(snapshot.complete(), snapshot.lines.iter().all(ScopedLine::exact));
            let saved = snapshot.clone();
            prop_assert_eq!(snapshot.complete(), saved.complete());
            let mut mutated = saved;
            for line in &mut mutated.lines {
                *line = if line.exact() { ScopedLine::default() } else { ScopedLine::empty(ScopeStackId(0)) };
            }
            // Legacy mutable consumers must not leave a stale cached count.
            let cloned_before_query = mutated.clone();
            prop_assert_eq!(mutated.complete(), mutated.lines.iter().all(ScopedLine::exact));
            prop_assert_eq!(mutated.complete(), cloned_before_query.complete());
        }
    }
}
