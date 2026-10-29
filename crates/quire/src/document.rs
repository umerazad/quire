use std::ops::Range;
use text::{Bias, Buffer, BufferId, ReplicaId};

pub struct Document {
    buffer: Buffer,
    version: u64,
}

impl Document {
    pub fn new(id: BufferId, text: &str) -> Self {
        Self {
            buffer: Buffer::new(ReplicaId::LOCAL, id, text),
            version: 0,
        }
    }

    pub fn text(&self) -> String {
        self.buffer.text()
    }

    pub fn version(&self) -> u64 {
        self.version
    }

    pub fn snapshot(&self) -> text::BufferSnapshot {
        self.buffer.snapshot().clone()
    }

    pub fn edit(
        &mut self,
        expected: u64,
        range: Range<usize>,
        inserted: &str,
    ) -> Result<u64, String> {
        if expected != self.version {
            return Err(
                "This page changed before the edit was applied. Refresh the selection and retry."
                    .into(),
            );
        }
        if range.start > range.end
            || range.end > self.buffer.len()
            || self.buffer.clip_offset(range.start, Bias::Left) != range.start
            || self.buffer.clip_offset(range.end, Bias::Left) != range.end
        {
            return Err("The edit range is outside the page or splits a Unicode character. Select the text again and retry.".into());
        }
        let next = self.version.checked_add(1).ok_or(
            "This page exhausted its edit sequence. Save and reopen the page before editing again.",
        )?;
        self.buffer.finalize_last_transaction();
        self.buffer.edit([(range, inserted)]);
        self.buffer.finalize_last_transaction();
        self.version = next;
        Ok(next)
    }

    pub fn undo(&mut self) -> bool {
        let Some(next) = self.version.checked_add(1) else {
            return false;
        };
        if self.buffer.undo().is_none() {
            return false;
        }
        self.version = next;
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(next) = self.version.checked_add(1) else {
            return false;
        };
        if self.buffer.redo().is_none() {
            return false;
        }
        self.version = next;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{AppContext, TestAppContext};
    use std::num::NonZeroU64;

    fn document(text: &str) -> Document {
        Document::new(text::BufferId::from(NonZeroU64::MIN), text)
    }

    #[gpui::test]
    fn views_share_edits_and_history(cx: &mut TestAppContext) {
        let source = cx.new(|_| document("- hello ^b-1\n"));
        let embed = source.clone();
        source.update(cx, |doc, _| doc.edit(0, 2..7, "مرحبا").unwrap());
        assert_eq!(embed.read_with(cx, |doc, _| doc.text()), "- مرحبا ^b-1\n");
        embed.update(cx, |doc, _| assert!(doc.undo()));
        assert_eq!(source.read_with(cx, |doc, _| doc.text()), "- hello ^b-1\n");
        source.update(cx, |doc, _| assert!(doc.redo()));
        assert_eq!(embed.read_with(cx, |doc, _| doc.text()), "- مرحبا ^b-1\n");
    }

    #[test]
    fn stale_edits_cannot_overwrite_later_changes_even_after_undo() {
        let mut doc = document("hello");
        doc.edit(0, 0..5, "first").unwrap();
        assert!(doc.edit(0, 0..5, "stale").is_err());
        assert_eq!(doc.text(), "first");
        assert!(doc.undo());
        assert_eq!(doc.text(), "hello");
        assert!(doc.edit(0, 0..5, "still stale").is_err());
    }

    #[test]
    fn malformed_ranges_do_not_mutate_the_document() {
        let mut doc = document("A🙂Z");
        for range in [2..3, Range { start: 5, end: 1 }, 0..99] {
            assert!(doc.edit(0, range, "oops").is_err());
            assert_eq!(doc.text(), "A🙂Z");
            assert_eq!(doc.version(), 0);
        }
    }

    #[test]
    fn commands_are_independent_undo_steps_and_new_edit_clears_redo() {
        let mut doc = document("a");
        doc.edit(0, 1..1, "b").unwrap();
        doc.edit(1, 2..2, "c").unwrap();
        assert!(doc.undo());
        assert_eq!(doc.text(), "ab");
        doc.edit(doc.version(), 2..2, "d").unwrap();
        assert!(!doc.redo());
        assert_eq!(doc.text(), "abd");
    }
}
