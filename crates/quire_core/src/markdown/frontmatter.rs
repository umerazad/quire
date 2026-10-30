use super::{frontmatter_length, js_space, page_name};
use crate::{
    json::{JsString, Json},
    source_text::normalized_line_units,
};
use std::{fmt::Write as _, ops::Range};

fn line_break(c: char) -> bool {
    matches!(c, '\r' | '\n' | '\u{2028}' | '\u{2029}')
}

fn body(text: &str) -> Option<&str> {
    let length = frontmatter_length(text);
    if length == 0 {
        return None;
    }
    let envelope = &text[..length];
    let closing = envelope
        .strip_suffix("\r\n")
        .or_else(|| envelope.strip_suffix('\n'))
        .unwrap_or(envelope);
    let before_marker = closing.strip_suffix("---")?;
    let before_break = before_marker
        .strip_suffix("\r\n")
        .or_else(|| before_marker.strip_suffix('\n'))?;
    Some(before_break.split_once('\n')?.1)
}

fn title_value_range(metadata: &str) -> Option<Range<usize>> {
    for (index, _) in metadata.match_indices("title:") {
        if index != 0
            && !metadata[..index]
                .chars()
                .next_back()
                .is_some_and(line_break)
        {
            continue;
        }
        let start = index + "title:".len();
        let rest = &metadata[start..];
        let value = rest.trim_start_matches(js_space);
        if !value.is_empty() {
            let from = start + rest.len() - value.len();
            return Some(from..from + value.find(line_break).unwrap_or(value.len()));
        }
        // Greedy whitespace backs up one non-line-terminator character to satisfy (.+).
        if let Some((offset, c)) = rest.char_indices().rev().find(|(_, c)| !line_break(*c)) {
            return Some(start + offset..start + offset + c.len_utf8());
        }
    }
    None
}

fn title_value(metadata: &str) -> Option<&str> {
    metadata.get(title_value_range(metadata)?)
}

fn unquote(value: &str) -> &str {
    let value = value.strip_prefix(['\'', '"']).unwrap_or(value);
    value.strip_suffix(['\'', '"']).unwrap_or(value)
}

/// Extracts the legacy title without losing UTF-16 code units or rewriting source.
pub fn page_title(path: &str, text: &str) -> JsString {
    if let Some(raw) = body(text).and_then(title_value) {
        if let Ok(value) = Json::parse(raw.as_bytes())
            && let Some(title) = value.javascript_string()
        {
            return title;
        }
        return unquote(raw).into();
    }
    page_name(path)
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .into()
}

/// Extracts a title from raw storage strings, including unpaired UTF-16 surrogates.
pub fn page_title_lossless(path: &JsString, text: &JsString) -> JsString {
    title_from_storage(path, text, false)
}

/// Uses the editor's LF view for analysis, preserving the original storage strings.
pub fn page_title_for_editor(path: &JsString, text: &JsString) -> JsString {
    title_from_storage(path, text, true)
}

fn title_from_storage(path: &JsString, text: &JsString, normalize: bool) -> JsString {
    if let (Some(path), Some(text)) = (path.as_str(), text.as_str())
        && (!normalize || !text.contains('\r'))
    {
        return page_title(path, text);
    }
    let mut mapped = String::new();
    let mut units = Vec::new();
    let mut boundaries = Vec::new();
    let length = super::frontmatter_length_chars(
        char::decode_utf16(normalized_line_units(text.utf16_units(), normalize)).map(|decoded| {
            boundaries.push((mapped.len(), units.len()));
            let c = match decoded {
                Ok(c) => {
                    let mut buffer = [0; 2];
                    units.extend_from_slice(c.encode_utf16(&mut buffer));
                    c
                }
                Err(error) => {
                    units.push(error.unpaired_surrogate());
                    '\u{fffd}'
                }
            };
            mapped.push(c);
            c
        }),
    );
    if length == 0 {
        return fallback_title(path);
    }
    boundaries.push((mapped.len(), units.len()));
    let raw = (|| {
        let metadata = body(&mapped)?;
        let range = title_value_range(metadata)?;
        let start = mapped.find('\n')? + 1;
        let from = boundaries
            .binary_search_by_key(&(start + range.start), |boundary| boundary.0)
            .ok()?;
        let to = boundaries
            .binary_search_by_key(&(start + range.end), |boundary| boundary.0)
            .ok()?;
        Some(&units[boundaries[from].1..boundaries[to].1])
    })();
    if let Some(raw) = raw {
        if let Some(value) = json_from_utf16(raw).and_then(|value| value.javascript_string()) {
            return value;
        }
        return JsString::from_utf16(unquote_units(raw));
    }
    fallback_title(path)
}

fn unquote_units(value: &[u16]) -> &[u16] {
    let quote = |unit: &u16| [u16::from(b'"'), u16::from(b'\'')].contains(unit);
    let value = match value.split_first() {
        Some((first, rest)) if quote(first) => rest,
        _ => value,
    };
    match value.split_last() {
        Some((last, rest)) if quote(last) => rest,
        _ => value,
    }
}

fn fallback_title(path: &JsString) -> JsString {
    let mut path: Vec<_> = path.utf16_units().collect();
    if let [.., dot, m, d] = path[..]
        && dot == u16::from(b'.')
        && [u16::from(b'M'), u16::from(b'm')].contains(&m)
        && [u16::from(b'D'), u16::from(b'd')].contains(&d)
    {
        path.truncate(path.len() - 3);
    }
    let from = path
        .iter()
        .rposition(|unit| *unit == u16::from(b'/'))
        .map_or(0, |index| index + 1);
    JsString::from_utf16(&path[from..])
}

fn json_from_utf16(units: &[u16]) -> Option<Json> {
    let mut text = String::new();
    let mut quoted = false;
    let mut escaped = false;
    for decoded in char::decode_utf16(units.iter().copied()) {
        match decoded {
            Ok(c) => {
                text.push(c);
                if escaped {
                    escaped = false;
                } else if quoted && c == '\\' {
                    escaped = true;
                } else if c == '"' {
                    quoted = !quoted;
                }
            }
            Err(error) => {
                if !quoted || escaped {
                    return None;
                }
                write!(text, "\\u{:04x}", error.unpaired_surrogate()).unwrap();
            }
        }
    }
    Json::parse(text.as_bytes()).ok()
}

fn tag_header(line: &str) -> Option<&str> {
    let (key, value) = line.split_once(':')?;
    let key = key.trim_end_matches([' ', '\t']);
    if !key.eq_ignore_ascii_case("tag") && !key.eq_ignore_ascii_case("tags") {
        return None;
    }
    let value = value.trim_start_matches([' ', '\t']);
    (!value.chars().any(line_break)).then_some(value)
}

fn list_value(line: &str) -> Option<&str> {
    let value = line.trim_start_matches([' ', '\t']).strip_prefix('-')?;
    value
        .starts_with([' ', '\t'])
        .then(|| value.trim_start_matches([' ', '\t']))
}

fn add_tag(value: &str, tags: &mut Vec<String>) {
    let value = unquote(value.trim_matches(js_space));
    let value = value
        .strip_prefix('#')
        .unwrap_or(value)
        .trim_matches(js_space);
    if !value.is_empty() {
        tags.push(value.to_owned());
    }
}

/// Reads the legacy inline/list tag forms, preserving their order and duplicates.
pub fn frontmatter_tags(text: &str) -> Vec<String> {
    let length = frontmatter_length(text);
    let mut lines = text[..length]
        .split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
        .peekable();
    let mut tags = Vec::new();
    while let Some(line) = lines.next() {
        let Some(inline) = tag_header(line) else {
            continue;
        };
        let inline = inline.trim_matches(js_space);
        if inline.is_empty() {
            while let Some(value) = lines.peek().and_then(|line| list_value(line)) {
                add_tag(value, &mut tags);
                lines.next();
            }
        } else {
            let inline = inline.strip_prefix('[').unwrap_or(inline);
            let inline = inline.strip_suffix(']').unwrap_or(inline);
            for value in inline.split(|c| c == ',' || js_space(c)) {
                add_tag(value, &mut tags);
            }
        }
    }
    tags
}
