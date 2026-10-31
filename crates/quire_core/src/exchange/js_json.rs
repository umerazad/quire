//! JSON as the reference app's JavaScript reads and writes it: `JSON.parse` into insertion-ordered
//! objects, and `JSON.stringify`, which lists integer-like keys first and prints numbers the way
//! JavaScript does.

use serde::Deserialize as _;
use serde_json::{Map, Value};
use std::fmt::Write as _;

/// Nesting beyond this is refused before parsing: rt accepts 129 levels of Roam blocks, two JSON
/// levels each, and attributes nest little.
const MAX_DEPTH: usize = 320;

pub const TOO_DEEP: &str = "Roam block children must be an array with at most 128 nesting levels. Export the graph again and retry.";

/// Parses JSON text into values whose objects keep their keys in first-seen order, as
/// `JSON.parse` does; a repeated key keeps its first position and takes the last value.
pub fn parse(text: &str) -> Result<Value, String> {
    if depth(text) > MAX_DEPTH {
        return Err(TOO_DEEP.to_owned());
    }
    let mut deserializer = serde_json::Deserializer::from_str(text);
    deserializer.disable_recursion_limit();
    let value = Value::deserialize(&mut deserializer).map_err(|error| error.to_string())?;
    deserializer.end().map_err(|error| error.to_string())?;
    Ok(value)
}

/// The deepest bracket nesting outside strings.
fn depth(text: &str) -> usize {
    let (mut depth, mut deepest, mut string, mut escaped) = (0usize, 0, false, false);
    for byte in text.bytes() {
        if string {
            match (escaped, byte) {
                (true, _) => escaped = false,
                (false, b'\\') => escaped = true,
                (false, b'"') => string = false,
                _ => {}
            }
            continue;
        }
        match byte {
            b'"' => string = true,
            b'[' | b'{' => {
                depth += 1;
                deepest = deepest.max(depth);
            }
            b']' | b'}' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    deepest
}

/// `JSON.stringify(value)`.
pub fn stringify(value: &Value) -> String {
    let mut output = String::new();
    write_value(&mut output, value, None, 0);
    output
}

/// `JSON.stringify(value, null, 2)`.
pub fn stringify_pretty(value: &Value) -> String {
    let mut output = String::new();
    write_value(&mut output, value, Some(2), 0);
    output
}

/// A JavaScript string literal as `JSON.stringify` writes it.
pub fn quote(text: &str) -> String {
    let mut output = String::with_capacity(text.len() + 2);
    write_string(&mut output, text);
    output
}

/// `String(number)` for a finite number.
pub fn number(value: f64) -> String {
    ryu_js::Buffer::new().format_finite(value).to_owned()
}

/// A JSON number holding `value`, which must be finite.
pub fn from_f64(value: f64) -> Value {
    serde_json::Number::from_f64(value).map_or(Value::Null, Value::Number)
}

/// The object's entries in JavaScript property order: array-index keys ascending, then the rest
/// in insertion order.
pub fn entries(map: &Map<String, Value>) -> Vec<(&String, &Value)> {
    let mut indexed: Vec<(u32, (&String, &Value))> = map
        .iter()
        .filter_map(|entry| array_index(entry.0).map(|index| (index, entry)))
        .collect();
    indexed.sort_by_key(|(index, _)| *index);
    indexed
        .into_iter()
        .map(|(_, entry)| entry)
        .chain(map.iter().filter(|(key, _)| array_index(key).is_none()))
        .collect()
}

fn array_index(key: &str) -> Option<u32> {
    let index: u32 = key.parse().ok()?;
    (index != u32::MAX && index.to_string() == key).then_some(index)
}

fn newline(output: &mut String, indent: Option<usize>, level: usize) {
    if let Some(width) = indent {
        output.push('\n');
        output.extend(std::iter::repeat_n(' ', width * level));
    }
}

fn write_value(output: &mut String, value: &Value, indent: Option<usize>, level: usize) {
    match value {
        Value::Null => output.push_str("null"),
        Value::Bool(value) => output.push_str(if *value { "true" } else { "false" }),
        Value::Number(value) => output.push_str(&number(value.as_f64().unwrap_or_default())),
        Value::String(value) => write_string(output, value),
        Value::Array(items) if items.is_empty() => output.push_str("[]"),
        Value::Array(items) => {
            output.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    output.push(',');
                }
                newline(output, indent, level + 1);
                write_value(output, item, indent, level + 1);
            }
            newline(output, indent, level);
            output.push(']');
        }
        Value::Object(map) if map.is_empty() => output.push_str("{}"),
        Value::Object(map) => {
            output.push('{');
            for (index, (key, item)) in entries(map).into_iter().enumerate() {
                if index > 0 {
                    output.push(',');
                }
                newline(output, indent, level + 1);
                write_string(output, key);
                output.push(':');
                if indent.is_some() {
                    output.push(' ');
                }
                write_value(output, item, indent, level + 1);
            }
            newline(output, indent, level);
            output.push('}');
        }
    }
}

fn write_string(output: &mut String, text: &str) {
    output.push('"');
    for character in text.chars() {
        match character {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\u{8}' => output.push_str("\\b"),
            '\u{c}' => output.push_str("\\f"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            control if control < ' ' => {
                write!(output, "\\u{:04x}", control as u32).ok();
            }
            other => output.push(other),
        }
    }
    output.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stringify_matches_javascript() {
        let value =
            parse(r#"{"b":1,"10":[],"2":{},"a":[1.5,-0,1e21,1e-7,"\u0001\n\"é"],"b":2}"#).unwrap();
        assert_eq!(
            stringify(&value),
            r#"{"2":{},"10":[],"b":2,"a":[1.5,0,1e+21,1e-7,"\u0001\n\"é"]}"#
        );
        assert_eq!(
            stringify_pretty(&value),
            "{\n  \"2\": {},\n  \"10\": [],\n  \"b\": 2,\n  \"a\": [\n    1.5,\n    0,\n    1e+21,\n    1e-7,\n    \"\\u0001\\n\\\"é\"\n  ]\n}"
        );
    }

    #[test]
    fn nesting_beyond_the_limit_is_refused_before_parsing() {
        let deep = format!("{}{}", "[".repeat(MAX_DEPTH + 1), "]".repeat(MAX_DEPTH + 1));
        assert_eq!(parse(&deep), Err(TOO_DEEP.to_owned()));
        let allowed = format!("{}{}", "[".repeat(MAX_DEPTH), "]".repeat(MAX_DEPTH));
        assert!(parse(&allowed).is_ok());
        assert!(parse(r#"["[[[[", "\"[["]"#).is_ok());
        assert!(
            parse(r#"["\ud800"]"#).is_err(),
            "a lone surrogate is refused"
        );
        assert!(parse(r#"["\ud83d\ude00"]"#).is_ok());
    }
}
