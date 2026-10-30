use super::{frontmatter_length, js_space, page_name};
use crate::json::{JsString, Json};

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

fn title_value(metadata: &str) -> Option<&str> {
    for (index, _) in metadata.match_indices("title:") {
        if index != 0
            && !metadata[..index]
                .chars()
                .next_back()
                .is_some_and(line_break)
        {
            continue;
        }
        let rest = &metadata[index + "title:".len()..];
        let value = rest.trim_start_matches(js_space);
        if !value.is_empty() {
            return Some(&value[..value.find(line_break).unwrap_or(value.len())]);
        }
        // Greedy whitespace backs up one non-line-terminator character to satisfy (.+).
        if let Some((offset, c)) = rest.char_indices().rev().find(|(_, c)| !line_break(*c)) {
            return Some(&rest[offset..offset + c.len_utf8()]);
        }
    }
    None
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
