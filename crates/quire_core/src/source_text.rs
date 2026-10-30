use std::{fmt, ops::Range, sync::Arc};

/// Original stored text and the byte translation for its LF-normalized editor representation.
pub struct SourceText {
    original: Arc<str>,
    crlf_ends: Vec<usize>,
    normalized_len: usize,
    preferred_newline: &'static str,
}

pub struct SourceEdit<S> {
    /// Byte offsets in the original LF-normalized representation.
    pub range: Range<usize>,
    /// Replacement text from the normalized editor, without carriage returns.
    pub replacement: S,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceEditError {
    InvalidRange,
    OverlappingEdits,
    UnnormalizedText,
}

impl fmt::Display for SourceEditError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let reason = match self {
            Self::InvalidRange => {
                "an edit is outside the opened text or splits a Unicode character"
            }
            Self::OverlappingEdits => "the edit ranges overlap or are out of order",
            Self::UnnormalizedText => {
                "the editor replacement contains an unnormalized carriage return"
            }
        };
        write!(
            f,
            "Cannot map the editor changes to stored source: {reason}. Keep the buffer and reopen the page before retrying."
        )
    }
}
impl std::error::Error for SourceEditError {}

impl SourceText {
    /// Run away from the foreground; both normalization and original-source ownership cost O(n).
    pub fn prepare(original: String) -> (Self, String) {
        let preferred_newline = match original.find(['\r', '\n']) {
            Some(index) if original[index..].starts_with("\r\n") => "\r\n",
            Some(index) if original.as_bytes()[index] == b'\r' => "\r",
            _ => "\n",
        };
        let normalized: String = normalized_chars(&original).collect();
        let crlf_ends = original
            .match_indices("\r\n")
            .enumerate()
            .map(|(removed, (start, _))| start + 1 - removed)
            .collect();
        (
            Self {
                original: original.into(),
                crlf_ends,
                normalized_len: normalized.len(),
                preferred_newline,
            },
            normalized,
        )
    }

    pub fn original(&self) -> Arc<str> {
        self.original.clone()
    }
    pub fn normalized_len(&self) -> usize {
        self.normalized_len
    }
    pub fn preferred_newline(&self) -> &'static str {
        self.preferred_newline
    }

    fn wire_offset(&self, normalized: usize) -> usize {
        normalized + self.crlf_ends.partition_point(|end| *end <= normalized)
    }

    /// Apply ordered edits relative to the opened representation, preserving untouched wire bytes.
    /// Semantically unchanged replacements retain their exact original line endings.
    /// New line breaks use the first opened line-break style, or LF for an empty/single-line page.
    /// Errors leave the immutable original untouched and return no partial result.
    pub fn apply<I, S>(&self, edits: I) -> Result<Arc<str>, SourceEditError>
    where
        I: IntoIterator<Item = SourceEdit<S>>,
        S: AsRef<str>,
    {
        let mut last_end = 0;
        let mut written = 0;
        let mut output: Option<String> = None;
        for edit in edits {
            if edit.range.start > edit.range.end || edit.range.end > self.normalized_len {
                return Err(SourceEditError::InvalidRange);
            }
            if edit.range.start < last_end {
                return Err(SourceEditError::OverlappingEdits);
            }
            last_end = edit.range.end;
            let start = self.wire_offset(edit.range.start);
            let end = self.wire_offset(edit.range.end);
            let original = self
                .original
                .get(start..end)
                .ok_or(SourceEditError::InvalidRange)?;
            let replacement = edit.replacement.as_ref();
            if replacement.contains('\r') {
                return Err(SourceEditError::UnnormalizedText);
            }
            if edit.range.len() == replacement.len()
                && normalized_chars(original).eq(replacement.chars())
            {
                continue;
            }
            let output = output.get_or_insert_with(|| String::with_capacity(self.original.len()));
            output.push_str(&self.original[written..start]);
            let mut lines = replacement.split('\n');
            output.push_str(lines.next().expect("split always yields one part"));
            for line in lines {
                output.push_str(self.preferred_newline);
                output.push_str(line);
            }
            written = end;
        }
        match output {
            None => Ok(self.original.clone()),
            Some(mut output) => {
                output.push_str(&self.original[written..]);
                Ok(output.into())
            }
        }
    }
}

fn normalized_chars(text: &str) -> impl Iterator<Item = char> + '_ {
    let mut chars = text.chars().peekable();
    std::iter::from_fn(move || {
        let next = chars.next()?;
        if next == '\r' {
            if chars.peek() == Some(&'\n') {
                chars.next();
            }
            Some('\n')
        } else {
            Some(next)
        }
    })
}
