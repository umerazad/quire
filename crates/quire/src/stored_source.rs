use gpui::{App, Entity};
use language::{Buffer, BufferSnapshot};
use quire_core::source_text::{SourceEdit, SourceText};
use std::sync::Arc;

pub struct PreparedSource {
    normalized: rope::Rope,
    original: Arc<SourceText>,
    line_ending: text::LineEnding,
}

#[derive(Clone)]
pub struct StoredSource {
    original: Arc<SourceText>,
    baseline: BufferSnapshot,
}

impl PreparedSource {
    /// Normalize and build the rope off the foreground before installing a native buffer.
    pub fn new(text: String) -> Self {
        let (original, normalized) = SourceText::prepare(text);
        let line_ending = if original.preferred_newline() == "\r\n" {
            text::LineEnding::Windows
        } else {
            text::LineEnding::Unix
        };
        Self {
            normalized: rope::Rope::from(normalized.as_str()),
            original: Arc::new(original),
            line_ending,
        }
    }

    /// Only buffer/model construction remains on the foreground; no full-source copy or normalization.
    pub fn install(self, cx: &mut App) -> (Entity<Buffer>, StoredSource) {
        let buffer = crate::markdown::prepared_buffer(self.normalized, self.line_ending, cx);
        let baseline = buffer.read(cx).snapshot();
        (
            buffer,
            StoredSource {
                original: self.original,
                baseline,
            },
        )
    }
}

impl StoredSource {
    /// Follows a merged version: `baseline` must hold exactly `original`'s normalized text.
    /// Older snapshots keep rendering through the adapter they were captured with.
    pub fn rebased(original: SourceText, baseline: BufferSnapshot) -> Self {
        debug_assert_eq!(original.normalized_len(), baseline.len());
        Self {
            original: Arc::new(original),
            baseline,
        }
    }

    pub fn original_len(&self) -> usize {
        self.original.original().len()
    }

    /// Render a captured snapshot away from the foreground. Keeps the opening baseline so undo
    /// can recover original mixed endings even after an intermediate save or recovery checkpoint.
    pub fn render(&self, current: &BufferSnapshot) -> Result<Arc<str>, String> {
        if current.remote_id() != self.baseline.remote_id() {
            return Err("This source snapshot belongs to another page. Keep the buffer and retry with the current page.".into());
        }
        if !current.version().observed_all(self.baseline.version()) {
            return Err("This source snapshot predates the opened page. Keep the buffer and retry with a current snapshot.".into());
        }
        if current.line_ending() != self.baseline.line_ending() {
            return Ok(current.line_ending().apply(current.text()).into());
        }
        self.original
            .apply(
                current
                    .edits_since::<usize>(self.baseline.version())
                    .map(|edit| SourceEdit {
                        range: edit.old,
                        replacement: current.text_for_range(edit.new).collect::<String>(),
                    }),
            )
            .map_err(|error| error.to_string())
    }
}

#[cfg(test)]
mod tests;
