//! Pure Quire Markdown metadata, independent of editor and storage runtimes.

use std::fmt;
use unicode_normalization::UnicodeNormalization as _;

mod frontmatter;
pub use frontmatter::{frontmatter_tags, page_title};

mod references;
pub use references::{Reference, ReferenceKind, ReferenceMedia, ReferenceSyntax, references};

pub fn page_name(path: &str) -> &str {
    if path.len() >= 3
        && path
            .get(path.len() - 3..)
            .is_some_and(|suffix| suffix.eq_ignore_ascii_case(".md"))
    {
        &path[..path.len() - 3]
    } else {
        path
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageNameError;

impl fmt::Display for PageNameError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("This page name cannot be used on Windows and macOS. Use a shorter name without a leading dot, slashes, or reserved filename characters.")
    }
}
impl std::error::Error for PageNameError {}

fn js_space(c: char) -> bool {
    (c.is_whitespace() && c != '\u{85}') || c == '\u{feff}'
}

/// Validates and NFC-normalizes a new page's single name using the legacy rules.
/// Existing notebook paths and contents must not be rewritten through this function.
pub fn safe_page_path(title: &str) -> Result<String, PageNameError> {
    let name: String = title.trim_matches(js_space).nfc().collect();
    let stem = name
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let reserved = matches!(stem.as_str(), "con" | "prn" | "aux" | "nul")
        || (stem.len() == 4
            && (stem.as_bytes().starts_with(b"com") || stem.as_bytes().starts_with(b"lpt"))
            && stem.as_bytes()[3].is_ascii_digit());
    if name.is_empty()
        || name.encode_utf16().count() > 150
        || name.starts_with('.')
        || name.ends_with(['.', ' '])
        || reserved
        || name
            .chars()
            .any(|c| c.is_control() || "<>:\"/\\|?*".contains(c))
    {
        return Err(PageNameError);
    }
    Ok(format!("{name}.md"))
}

fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn decode_uri_component(input: &str) -> Option<String> {
    let mut decoded = Vec::with_capacity(input.len());
    let mut bytes = input.bytes();
    while let Some(byte) = bytes.next() {
        decoded.push(if byte == b'%' {
            hex(bytes.next()?)? * 16 + hex(bytes.next()?)?
        } else {
            byte
        });
    }
    String::from_utf8(decoded).ok()
}

/// Resolves a logical notebook link, rejecting malformed URI escapes and traversal above root.
/// This is not filesystem authorization. The file adapter must validate the resulting path.
pub fn relative_vault_path(from: Option<&str>, target: &str) -> Option<String> {
    let decoded = decode_uri_component(target.split('#').next().unwrap_or_default())?;
    let mut parts = if decoded.starts_with('/') {
        Vec::new()
    } else {
        from.map(|path| {
            let mut parts: Vec<_> = path.split('/').collect();
            parts.pop();
            parts
        })
        .unwrap_or_default()
    };
    for part in decoded.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            _ => parts.push(part),
        }
    }
    Some(parts.join("/"))
}

/// Returns the byte length of the leading legacy YAML envelope, without normalizing source.
pub fn frontmatter_length(text: &str) -> usize {
    frontmatter_length_chars(text.chars())
}

/// Scans borrowed rope characters without allocating page or line strings. Returns bytes.
pub fn frontmatter_length_chars(chars: impl IntoIterator<Item = char>) -> usize {
    let mut chars = chars.into_iter().peekable();
    let mut offset = 0;
    if chars.peek() == Some(&'\u{feff}') {
        chars.next();
        offset += '\u{feff}'.len_utf8();
    }
    for _ in 0..3 {
        if chars.next() != Some('-') {
            return 0;
        }
        offset += 1;
    }
    offset += match chars.next() {
        Some('\n') => 1,
        Some('\r') if chars.next() == Some('\n') => 2,
        _ => return 0,
    };
    let mut after_body_newline = false;
    let mut dashes = 0;
    let mut delimiter = true;
    let mut carriage_return = false;
    for c in chars {
        offset += c.len_utf8();
        if c == '\n' {
            if after_body_newline && delimiter && dashes == 3 {
                return offset;
            }
            after_body_newline = true;
            dashes = 0;
            delimiter = true;
            carriage_return = false;
        } else if c == '-' && dashes < 3 && !carriage_return {
            dashes += 1;
        } else if c == '\r' && dashes == 3 && !carriage_return {
            carriage_return = true;
        } else {
            delimiter = false;
        }
    }
    if after_body_newline && delimiter && dashes == 3 && !carriage_return {
        offset
    } else {
        0
    }
}
