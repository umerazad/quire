use quire_core::source_text::{SourceEdit, SourceText};
use std::sync::Arc;

#[test]
fn preparation_matches_native_normalization_and_retains_the_original() {
    for (raw, normalized, preferred) in [
        ("", "", "\n"),
        ("plain اردو 🙂", "plain اردو 🙂", "\n"),
        ("one\n", "one\n", "\n"),
        ("one\r\n", "one\n", "\r\n"),
        ("one\rtwo\r", "one\ntwo\n", "\r"),
        ("عربي\r\nUrdu\r🙂\n\r\n", "عربي\nUrdu\n🙂\n\n", "\r\n"),
        ("\r\r\n\n\r", "\n\n\n\n", "\r"),
    ] {
        let (source, text) = SourceText::prepare(raw.into());
        assert_eq!(text, normalized);
        assert_eq!(&*source.original(), raw);
        assert_eq!(source.preferred_newline(), preferred);
        assert_eq!(source.normalized_len(), normalized.len());
        assert!(Arc::ptr_eq(
            &source.original(),
            &source
                .apply(std::iter::empty::<SourceEdit<&str>>())
                .unwrap()
        ));
    }
}

#[test]
fn sparse_edits_preserve_mixed_endings_and_use_the_opened_style_for_insertions() {
    let (source, _) = SourceText::prepare("A\r\nB\rC\nD\r\n".into());
    let changed = source
        .apply([
            SourceEdit {
                range: 0..1,
                replacement: "عربي",
            },
            SourceEdit {
                range: 2..3,
                replacement: "اردو",
            },
            SourceEdit {
                range: 4..4,
                replacement: "🙂\n",
            },
            SourceEdit {
                range: 6..7,
                replacement: "last",
            },
        ])
        .unwrap();
    assert_eq!(&*changed, "عربي\r\nاردو\r🙂\r\nC\nlast\r\n");
    assert_eq!(&*source.original(), "A\r\nB\rC\nD\r\n");
}

#[test]
fn normalized_noop_replacements_do_not_rewrite_wire_endings() {
    let (source, normalized) = SourceText::prepare("A\r\nB\rC\n🙂\r\n".into());
    for edits in [
        vec![SourceEdit {
            range: 0..normalized.len(),
            replacement: normalized.as_str(),
        }],
        vec![
            SourceEdit {
                range: 1..2,
                replacement: "\n",
            },
            SourceEdit {
                range: 3..4,
                replacement: "\n",
            },
        ],
    ] {
        assert!(Arc::ptr_eq(
            &source.original(),
            &source.apply(edits).unwrap()
        ));
    }
    let changed = source
        .apply([
            SourceEdit {
                range: 1..2,
                replacement: "\n",
            },
            SourceEdit {
                range: 4..5,
                replacement: "changed",
            },
        ])
        .unwrap();
    assert_eq!(&*changed, "A\r\nB\rchanged\n🙂\r\n");
}

#[test]
fn edits_at_newline_boundaries_and_the_end_map_to_complete_wire_sequences() {
    for (raw, range, inserted, expected) in [
        ("A\r\nB", 1..2, "", "AB"),
        ("A\r\nB", 1..1, "x", "Ax\r\nB"),
        ("A\r\nB", 2..2, "x", "A\r\nxB"),
        ("A\r\nB", 3..3, "\nend", "A\r\nB\r\nend"),
        ("A\rB", 1..2, "\n\n", "A\r\rB"),
        ("A\nB", 0..3, "🙂\n", "🙂\n"),
        ("\r\n", 0..1, "", ""),
        ("", 0..0, "🙂\n", "🙂\n"),
    ] {
        let (source, _) = SourceText::prepare(raw.into());
        assert_eq!(
            &*source
                .apply([SourceEdit {
                    range,
                    replacement: inserted
                }])
                .unwrap(),
            expected
        );
    }
}

#[test]
fn invalid_edits_never_change_the_original_or_return_a_partial_rewrite() {
    let (source, _) = SourceText::prepare("🙂\r\nB\rC".into());
    for edits in [
        vec![SourceEdit {
            range: 1..2,
            replacement: "split",
        }],
        vec![SourceEdit {
            range: 0..99,
            replacement: "outside",
        }],
        vec![SourceEdit {
            range: std::ops::Range { start: 5, end: 4 },
            replacement: "reversed",
        }],
        vec![SourceEdit {
            range: 0..1,
            replacement: "split end",
        }],
        vec![
            SourceEdit {
                range: 0..0,
                replacement: "valid first",
            },
            SourceEdit {
                range: 1..2,
                replacement: "invalid later",
            },
        ],
        vec![
            SourceEdit {
                range: 5..6,
                replacement: "later",
            },
            SourceEdit {
                range: 0..0,
                replacement: "earlier",
            },
        ],
        vec![
            SourceEdit {
                range: 0..5,
                replacement: "first",
            },
            SourceEdit {
                range: 4..6,
                replacement: "overlap",
            },
        ],
        vec![SourceEdit {
            range: 0..0,
            replacement: "raw\r\n",
        }],
    ] {
        assert!(source.apply(edits).is_err());
        assert_eq!(&*source.original(), "🙂\r\nB\rC");
    }
    let changed = source
        .apply([SourceEdit {
            range: 5..6,
            replacement: "valid",
        }])
        .unwrap();
    assert_eq!(&*changed, "🙂\r\nvalid\rC");
}
