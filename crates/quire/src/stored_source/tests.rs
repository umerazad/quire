use super::*;
use gpui::TestAppContext;

fn open(raw: &str, cx: &mut TestAppContext) -> (Entity<Buffer>, StoredSource) {
    let prepared = PreparedSource::new(raw.into());
    cx.update(|cx| prepared.install(cx))
}

fn capture(buffer: &Entity<Buffer>, cx: &mut TestAppContext) -> BufferSnapshot {
    buffer.read_with(cx, |buffer, _| buffer.snapshot())
}

fn edit(
    buffer: &Entity<Buffer>,
    range: std::ops::Range<usize>,
    text: &str,
    cx: &mut TestAppContext,
) {
    buffer.update(cx, |buffer, cx| {
        buffer.finalize_last_transaction();
        buffer.edit([(range, text)], None, cx);
        buffer.finalize_last_transaction();
    });
}

#[gpui::test]
fn native_snapshot_edits_preserve_wire_endings_and_full_undo(cx: &mut TestAppContext) {
    cx.update(|cx| crate::editor_host::init(cx).unwrap());
    let raw = "A\r\nB\rC\n🙂\r\n";
    let (buffer, stored) = open(raw, cx);
    assert_eq!(capture(&buffer, cx).text(), "A\nB\nC\n🙂\n");
    let original = stored.render(&capture(&buffer, cx)).unwrap();
    assert_eq!(&*original, raw);
    edit(&buffer, 2..3, "اردو", cx);
    let first = capture(&buffer, cx);
    let first_wire = stored.render(&first).unwrap();
    assert_eq!(&*first_wire, "A\r\nاردو\rC\n🙂\r\n");
    edit(&buffer, 0..2, "", cx);
    assert_eq!(
        &*stored.render(&capture(&buffer, cx)).unwrap(),
        "اردو\rC\n🙂\r\n"
    );
    // A request owns its captured version even while later typing continues.
    assert_eq!(&*stored.render(&first).unwrap(), &*first_wire);
    buffer.update(cx, |buffer, cx| {
        buffer.undo(cx).unwrap();
    });
    assert_eq!(
        &*stored.render(&capture(&buffer, cx)).unwrap(),
        &*first_wire
    );
    buffer.update(cx, |buffer, cx| {
        buffer.undo(cx).unwrap();
    });
    assert!(Arc::ptr_eq(
        &original,
        &stored.render(&capture(&buffer, cx)).unwrap()
    ));
    buffer.update(cx, |buffer, cx| {
        buffer.redo(cx).unwrap();
    });
    assert_eq!(
        &*stored.render(&capture(&buffer, cx)).unwrap(),
        &*first_wire
    );
}

#[gpui::test]
fn mapping_uses_real_native_edits_for_all_three_line_break_styles(cx: &mut TestAppContext) {
    cx.update(|cx| crate::editor_host::init(cx).unwrap());
    for a in ["\n", "\r\n", "\r"] {
        for b in ["\n", "\r\n", "\r"] {
            for c in ["\n", "\r\n", "\r"] {
                let raw = format!("a{a}عربي{b}🙂{c}");
                let mut expected_normalized = raw.clone();
                text::LineEnding::normalize(&mut expected_normalized);
                let (buffer, stored) = open(&raw, cx);
                assert_eq!(capture(&buffer, cx).text(), expected_normalized);
                edit(&buffer, 2..10, "اردو\nnew", cx);
                let wire = stored.render(&capture(&buffer, cx)).unwrap();
                assert_eq!(&*wire, format!("a{a}اردو{a}new{b}🙂{c}"));
                let mut normalized = wire.to_string();
                text::LineEnding::normalize(&mut normalized);
                assert_eq!(normalized, capture(&buffer, cx).text());
                buffer.update(cx, |buffer, cx| {
                    buffer.undo(cx).unwrap();
                });
                assert_eq!(&*stored.render(&capture(&buffer, cx)).unwrap(), raw);
            }
        }
    }
}

#[gpui::test]
fn foreign_and_predating_snapshots_are_rejected_before_mapping(cx: &mut TestAppContext) {
    cx.update(|cx| crate::editor_host::init(cx).unwrap());
    let (first, stored) = open("same\r\n", cx);
    let (second, _) = open("same\r\n", cx);
    assert!(
        stored
            .render(&capture(&second, cx))
            .unwrap_err()
            .contains("another page")
    );
    let earlier = capture(&first, cx);
    edit(&first, 0..0, "new", cx);
    let mut newer_baseline = stored.clone();
    newer_baseline.baseline = capture(&first, cx);
    assert!(
        newer_baseline
            .render(&earlier)
            .unwrap_err()
            .contains("predates")
    );
    assert_eq!(&*stored.render(&earlier).unwrap(), "same\r\n");
}

#[gpui::test]
fn an_explicit_native_line_ending_change_is_distinct_from_ordinary_text_edits(
    cx: &mut TestAppContext,
) {
    cx.update(|cx| crate::editor_host::init(cx).unwrap());
    let (buffer, stored) = open("a\nb\rc\n", cx);
    buffer.update(cx, |buffer, cx| {
        buffer.set_line_ending(text::LineEnding::Windows, cx)
    });
    assert_eq!(
        &*stored.render(&capture(&buffer, cx)).unwrap(),
        "a\r\nb\r\nc\r\n"
    );
    buffer.update(cx, |buffer, cx| {
        buffer.set_line_ending(text::LineEnding::Unix, cx)
    });
    assert_eq!(&*stored.render(&capture(&buffer, cx)).unwrap(), "a\nb\rc\n");
}

#[gpui::test]
fn repeated_unicode_edits_and_history_replay_keep_source_mapping_stable(cx: &mut TestAppContext) {
    cx.update(|cx| crate::editor_host::init(cx).unwrap());
    let (buffer, stored) = open("- عربي\r\n- اردو\r- 🙂\n", cx);
    let original = stored.render(&capture(&buffer, cx)).unwrap();
    let mut seed = 37u64;
    for index in 0..120 {
        let text = capture(&buffer, cx).text();
        let boundaries: Vec<_> = text
            .char_indices()
            .map(|(offset, _)| offset)
            .chain(std::iter::once(text.len()))
            .collect();
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        let a = boundaries[(seed as usize) % boundaries.len()];
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        let b = boundaries[(seed as usize) % boundaries.len()];
        edit(
            &buffer,
            a.min(b)..a.max(b),
            ["", "🙂", "اردو\n", "\n\n", "x\u{301}"][index % 5],
            cx,
        );
        let current = capture(&buffer, cx);
        let mut normalized = stored.render(&current).unwrap().to_string();
        text::LineEnding::normalize(&mut normalized);
        assert_eq!(normalized, current.text(), "operation {index}");
    }
    let latest = stored.render(&capture(&buffer, cx)).unwrap();
    buffer.update(cx, |buffer, cx| while buffer.undo(cx).is_some() {});
    assert!(Arc::ptr_eq(
        &original,
        &stored.render(&capture(&buffer, cx)).unwrap()
    ));
    buffer.update(cx, |buffer, cx| while buffer.redo(cx).is_some() {});
    assert_eq!(&*stored.render(&capture(&buffer, cx)).unwrap(), &*latest);
}
