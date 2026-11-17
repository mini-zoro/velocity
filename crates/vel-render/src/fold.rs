//! Fold projection independent of the editor, font system and GPU.
use std::ops::Range;

/// Projects a nested fold forest to its visible outer intervals. Overlapping
/// intervals share one marker; children remain in the editor's fold forest.
pub fn outermost(ranges: impl IntoIterator<Item = Range<usize>>) -> Vec<Range<usize>> {
    let mut ranges: Vec<_> = ranges.into_iter().filter(|r| !r.is_empty()).collect();
    ranges.sort_unstable_by_key(|r| (r.start, std::cmp::Reverse(r.end)));
    let mut visible: Vec<Range<usize>> = Vec::new();
    for range in ranges {
        if let Some(last) = visible.last_mut()
            && range.start < last.end
        {
            last.end = last.end.max(range.end);
        } else {
            visible.push(range);
        }
    }
    visible
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_crossing_adjacent_and_empty_intervals() {
        assert_eq!(
            outermost([4..8, 0..20, 20..22, 3..10, 22..22]),
            [0..20, 20..22]
        );
        assert_eq!(
            outermost([2..6, 4..10, 3..5]),
            std::iter::once(2..10).collect::<Vec<_>>()
        );
        let range = 4..8;
        assert_eq!(
            outermost(std::iter::once(range.clone())),
            std::iter::once(range).collect::<Vec<_>>()
        );
    }
}
