use std::ops::Range;

pub(super) fn merged_ranges(ranges: impl Iterator<Item = Range<usize>>) -> Vec<Range<usize>> {
    let mut ranges: Vec<_> = ranges.filter(|range| range.start < range.end).collect();
    ranges.sort_unstable_by_key(|range| range.start);
    let mut merged: Vec<Range<usize>> = Vec::new();
    for range in ranges {
        if let Some(previous) = merged.last_mut()
            && range.start <= previous.end
        {
            previous.end = previous.end.max(range.end);
        } else {
            merged.push(range);
        }
    }
    merged
}

pub(super) fn contains(ranges: &[Range<usize>], position: usize) -> bool {
    let index = ranges.partition_point(|range| range.start <= position);
    index > 0 && position < ranges[index - 1].end
}

pub(super) fn overlaps(ranges: &[Range<usize>], query: &Range<usize>) -> bool {
    let index = ranges.partition_point(|range| range.start < query.end);
    index > 0 && ranges[index - 1].end > query.start
}
