//! The words of a page that spelling checks: Unicode words (UAX #29) of two or more letters in
//! prose, outside the page analysis's gaps. Words holding digits, underscores, dots, or other
//! joiners are names, versions, or file names rather than prose, and are left alone, as is a
//! block ID after its `^`.

use std::ops::Range;
use unicode_segmentation::UnicodeSegmentation as _;

const APOSTROPHES: [char; 2] = ['\'', '\u{2019}'];

/// The prose words of `text`, which starts at byte `base` of the page, as page byte ranges with
/// their spelling. `gaps` are sorted, disjoint page ranges that are not prose; a word overlapping one
/// is skipped. The word holding `hold` (inclusive at its end) is skipped too, as it is still being
/// typed.
pub fn prose<'a>(
    text: &'a str,
    base: usize,
    gaps: &[Range<usize>],
    hold: Option<usize>,
) -> Vec<(Range<usize>, &'a str)> {
    text.split_word_bound_indices()
        .filter_map(|(at, segment)| {
            let word = segment.trim_matches(&APOSTROPHES[..]);
            let start = at + (segment.len() - segment.trim_start_matches(&APOSTROPHES[..]).len());
            let range = base + start..base + start + word.len();
            let letters = word.chars().filter(|c| c.is_alphabetic()).count();
            let prose = letters >= 2
                && word
                    .chars()
                    .all(|c| c.is_alphabetic() || APOSTROPHES.contains(&c))
                && !text[..start].ends_with('^')
                && !overlaps(gaps, &range)
                && !hold.is_some_and(|hold| range.start < hold && hold <= range.end);
            prose.then_some((range, word))
        })
        .collect()
}

/// Whether `range` overlaps a gap. A word beside one, as between HTML tags, is still prose.
fn overlaps(gaps: &[Range<usize>], range: &Range<usize>) -> bool {
    let after = gaps.partition_point(|gap| gap.end <= range.start);
    gaps.get(after).is_some_and(|gap| gap.start < range.end)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words<'a>(text: &'a str, gaps: &[Range<usize>], hold: Option<usize>) -> Vec<&'a str> {
        prose(text, 0, gaps, hold)
            .into_iter()
            .map(|(_, word)| word)
            .collect()
    }

    #[test]
    fn prose_words_leave_out_names_numbers_ids_and_single_letters() {
        assert_eq!(
            words(
                "Teh café's v2 file.md snake_case a I don't ^blockid 'quoted' naïve",
                &[],
                None
            ),
            ["Teh", "café's", "don't", "quoted", "naïve"]
        );
    }

    #[test]
    fn prose_words_skip_gaps_and_the_word_being_typed() {
        let text = "keep `codez` skip wurd";
        let code = text.find('`').unwrap()..text.find(" skip").unwrap();
        assert_eq!(words(text, &[code], None), ["keep", "skip", "wurd"]);
        assert_eq!(
            words(text, &[], Some(text.len())),
            ["keep", "codez", "skip"]
        );
        assert_eq!(words(text, &[], Some(2)), ["codez", "skip", "wurd"]);
        assert_eq!(words(text, &[], Some(0)), ["keep", "codez", "skip", "wurd"]);
        assert_eq!(
            words("<b>boldd</b>", &[0..3, 8..12], None),
            ["boldd"],
            "a word between HTML tags is prose"
        );
    }

    #[test]
    fn prose_ranges_are_page_offsets() {
        assert_eq!(
            prose("x ''wrod''", 10, &[], None),
            [(14..18, "wrod")],
            "apostrophes around a word are quotes"
        );
    }
}
