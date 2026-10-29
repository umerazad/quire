//! Lossless JavaScript JSON for the sync boundary, independent of editor strings.
//! serde_json validates grammar and decodes strings to WTF-8 bytes. The flat arena
//! keeps parsing, cloning, canonical serialization, and destruction iterative.

use serde::{Deserialize, Deserializer, de::Visitor};
use serde_json::value::RawValue;
use std::{cmp::Ordering, collections::BTreeMap, fmt, fmt::Write};

#[derive(Clone, Debug)]
pub struct JsString(Vec<u8>);

impl JsString {
    pub(crate) fn as_bytes(&self) -> &[u8] {
        &self.0
    }
    pub fn as_str(&self) -> Option<&str> {
        std::str::from_utf8(&self.0).ok()
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    fn units(&self) -> Utf16<'_> {
        Utf16 {
            bytes: &self.0,
            trailing: None,
        }
    }

    fn write_json(&self, output: &mut String) {
        if let Some(text) = self.as_str() {
            output.push_str(&serde_json::to_string(text).unwrap());
            return;
        }
        output.push('"');
        for decoded in char::decode_utf16(self.units()) {
            match decoded {
                Ok('"') => output.push_str("\\\""),
                Ok('\\') => output.push_str("\\\\"),
                Ok('\u{8}') => output.push_str("\\b"),
                Ok('\u{c}') => output.push_str("\\f"),
                Ok('\n') => output.push_str("\\n"),
                Ok('\r') => output.push_str("\\r"),
                Ok('\t') => output.push_str("\\t"),
                Ok(ch) if ch <= '\u{1f}' => write!(output, "\\u{:04x}", ch as u32).unwrap(),
                Ok(ch) => output.push(ch),
                Err(error) => write!(output, "\\u{:04x}", error.unpaired_surrogate()).unwrap(),
            }
        }
        output.push('"');
    }
}

impl From<&str> for JsString {
    fn from(value: &str) -> Self {
        Self(value.as_bytes().to_vec())
    }
}
impl Ord for JsString {
    fn cmp(&self, other: &Self) -> Ordering {
        self.units().cmp(other.units())
    }
}
impl PartialEq for JsString {
    fn eq(&self, other: &Self) -> bool {
        self.units().eq(other.units())
    }
}
impl Eq for JsString {}
impl PartialOrd for JsString {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl<'de> Deserialize<'de> for JsString {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Bytes;
        impl<'de> Visitor<'de> for Bytes {
            type Value = JsString;
            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a JSON string")
            }
            fn visit_bytes<E: serde::de::Error>(self, bytes: &[u8]) -> Result<JsString, E> {
                let mut remaining = bytes;
                while let Err(error) = std::str::from_utf8(remaining) {
                    let invalid = &remaining[error.valid_up_to()..];
                    if invalid.first() != Some(&0xed)
                        || !invalid.get(1).is_some_and(|b| (0xa0..=0xbf).contains(b))
                        || !invalid.get(2).is_some_and(|b| (0x80..=0xbf).contains(b))
                    {
                        return Err(E::custom("JSON string contains invalid WTF-8 bytes"));
                    }
                    remaining = &invalid[3..];
                }
                Ok(JsString(bytes.to_vec()))
            }
        }
        deserializer.deserialize_bytes(Bytes)
    }
}

struct Utf16<'a> {
    bytes: &'a [u8],
    trailing: Option<u16>,
}
impl Iterator for Utf16<'_> {
    type Item = u16;
    fn next(&mut self) -> Option<u16> {
        if let Some(unit) = self.trailing.take() {
            return Some(unit);
        }
        let first = *self.bytes.first()?;
        let (width, mask) = match first {
            0..=0x7f => (1, 0x7f),
            0xc0..=0xdf => (2, 0x1f),
            0xe0..=0xef => (3, 0x0f),
            _ => (4, 0x07),
        };
        let mut codepoint = u32::from(first & mask);
        for byte in &self.bytes[1..width] {
            codepoint = (codepoint << 6) | u32::from(byte & 0x3f);
        }
        self.bytes = &self.bytes[width..];
        if codepoint <= 0xffff {
            Some(codepoint as u16)
        } else {
            let codepoint = codepoint - 0x10000;
            self.trailing = Some(0xdc00 + (codepoint & 0x3ff) as u16);
            Some(0xd800 + (codepoint >> 10) as u16)
        }
    }
}

#[derive(Clone, Debug)]
enum Node {
    Null,
    Bool(bool),
    Number(f64),
    String(JsString),
    Array(Vec<usize>),
    Object(BTreeMap<JsString, usize>),
}

#[derive(Clone, Debug)]
pub struct Json {
    nodes: Vec<Node>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct NonFiniteNumber;
impl fmt::Display for NonFiniteNumber {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Change records cannot hold infinite numbers.")
    }
}
impl std::error::Error for NonFiniteNumber {}

#[derive(Clone, Copy, Debug)]
pub struct ValueRef<'a> {
    json: &'a Json,
    index: usize,
}

impl Json {
    pub fn parse(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        // Buffer.toString("utf8") in the source reader replaces malformed UTF-8.
        let text = String::from_utf8_lossy(bytes);
        // Validate before scanning tokens; the scanner relies on complete strings and object keys.
        let raw: &RawValue = serde_json::from_str(&text)?;
        let text = raw.get();
        let bytes = text.as_bytes();
        let mut json = Self { nodes: Vec::new() };
        let mut frames: Vec<(usize, Option<JsString>)> = Vec::new();
        let mut offset = 0;
        while offset < bytes.len() {
            let byte = bytes[offset];
            if byte.is_ascii_whitespace() || matches!(byte, b',' | b':') {
                offset += 1;
                continue;
            }
            if matches!(byte, b']' | b'}') {
                frames.pop();
                offset += 1;
                continue;
            }
            let node = match byte {
                b'{' => {
                    offset += 1;
                    Node::Object(BTreeMap::new())
                }
                b'[' => {
                    offset += 1;
                    Node::Array(Vec::new())
                }
                b'"' => {
                    let start = offset;
                    offset += 1;
                    while bytes[offset] != b'"' {
                        offset += if bytes[offset] == b'\\' { 2 } else { 1 };
                    }
                    offset += 1;
                    let string: JsString = serde_json::from_str(&text[start..offset])?;
                    if let Some((parent, key)) = frames.last_mut()
                        && matches!(json.nodes[*parent], Node::Object(_))
                        && key.is_none()
                    {
                        *key = Some(string);
                        continue;
                    }
                    Node::String(string)
                }
                b'n' => {
                    offset += 4;
                    Node::Null
                }
                b't' => {
                    offset += 4;
                    Node::Bool(true)
                }
                b'f' => {
                    offset += 5;
                    Node::Bool(false)
                }
                _ => {
                    let start = offset;
                    while offset < bytes.len()
                        && !bytes[offset].is_ascii_whitespace()
                        && !matches!(bytes[offset], b',' | b']' | b'}')
                    {
                        offset += 1;
                    }
                    Node::Number(text[start..offset].parse().expect("validated JSON number"))
                }
            };
            let container = matches!(node, Node::Array(_) | Node::Object(_));
            let index = json.nodes.len();
            json.nodes.push(node);
            if let Some((parent, key)) = frames.last_mut() {
                match &mut json.nodes[*parent] {
                    Node::Array(children) => children.push(index),
                    Node::Object(children) => {
                        children.insert(key.take().expect("validated object key"), index);
                    }
                    _ => unreachable!("validated JSON container"),
                }
            }
            if container {
                frames.push((index, None));
            }
        }
        Ok(json)
    }

    pub fn root(&self) -> ValueRef<'_> {
        ValueRef {
            json: self,
            index: 0,
        }
    }
    pub fn canonical(&self) -> Result<String, NonFiniteNumber> {
        self.root().canonical()
    }

    pub(crate) fn set_number(&mut self, key: &str, number: f64) -> bool {
        if !matches!(self.nodes.first(), Some(Node::Object(_))) {
            return false;
        }
        let index = self.nodes.len();
        self.nodes.push(Node::Number(number));
        if let Node::Object(fields) = &mut self.nodes[0] {
            fields.insert(key.into(), index);
        }
        true
    }
}

impl From<&serde_json::Value> for Json {
    fn from(value: &serde_json::Value) -> Self {
        Self::parse(&serde_json::to_vec(value).expect("serializable JSON value"))
            .expect("serialized JSON")
    }
}
impl From<&Json> for Json {
    fn from(value: &Json) -> Self {
        value.clone()
    }
}

impl<'a> ValueRef<'a> {
    fn node(self) -> &'a Node {
        self.json.nodes.get(self.index).unwrap_or(&Node::Null)
    }
    pub fn get(self, key: &str) -> Option<Self> {
        let Node::Object(fields) = self.node() else {
            return None;
        };
        fields.get(&JsString::from(key)).map(|&index| Self {
            json: self.json,
            index,
        })
    }
    pub fn field(self, key: &str) -> Self {
        self.get(key).unwrap_or(Self {
            json: self.json,
            index: usize::MAX,
        })
    }
    pub fn as_str(self) -> Option<&'a str> {
        self.as_js_string().and_then(JsString::as_str)
    }
    pub fn as_js_string(self) -> Option<&'a JsString> {
        if let Node::String(value) = self.node() {
            Some(value)
        } else {
            None
        }
    }
    pub fn as_f64(self) -> Option<f64> {
        if let Node::Number(value) = self.node() {
            Some(*value)
        } else {
            None
        }
    }
    pub fn as_bool(self) -> Option<bool> {
        if let Node::Bool(value) = self.node() {
            Some(*value)
        } else {
            None
        }
    }
    pub fn is_null(self) -> bool {
        matches!(self.node(), Node::Null)
    }
    pub fn is_string(self) -> bool {
        matches!(self.node(), Node::String(_))
    }
    pub fn is_boolean(self) -> bool {
        matches!(self.node(), Node::Bool(_))
    }
    pub fn is_object(self) -> bool {
        matches!(self.node(), Node::Object(_))
    }
    pub fn is_array(self) -> bool {
        matches!(self.node(), Node::Array(_))
    }
    pub fn elements(self) -> Option<impl DoubleEndedIterator<Item = Self> + ExactSizeIterator> {
        let Node::Array(children) = self.node() else {
            return None;
        };
        Some(children.iter().map(move |&index| Self {
            json: self.json,
            index,
        }))
    }
    pub fn members(
        self,
    ) -> Option<impl DoubleEndedIterator<Item = (&'a JsString, Self)> + ExactSizeIterator> {
        let Node::Object(children) = self.node() else {
            return None;
        };
        Some(children.iter().map(move |(name, &index)| {
            (
                name,
                Self {
                    json: self.json,
                    index,
                },
            )
        }))
    }
    pub fn canonical(self) -> Result<String, NonFiniteNumber> {
        enum Task<'a> {
            Value(usize),
            Key(&'a JsString),
            Text(&'static str),
        }
        let mut pending = vec![Task::Value(self.index)];
        let mut output = String::new();
        while let Some(task) = pending.pop() {
            match task {
                Task::Key(string) => string.write_json(&mut output),
                Task::Text(text) => output.push_str(text),
                Task::Value(index) => match self.json.nodes.get(index).unwrap_or(&Node::Null) {
                    Node::Null => output.push_str("null"),
                    Node::Bool(value) => output.push_str(if *value { "true" } else { "false" }),
                    Node::Number(value) => {
                        if !value.is_finite() {
                            return Err(NonFiniteNumber);
                        }
                        output.push_str(ryu_js::Buffer::new().format_finite(*value))
                    }
                    Node::String(value) => value.write_json(&mut output),
                    Node::Array(values) => {
                        output.push('[');
                        pending.push(Task::Text("]"));
                        for (i, &value) in values.iter().enumerate().rev() {
                            pending.push(Task::Value(value));
                            if i > 0 {
                                pending.push(Task::Text(","));
                            }
                        }
                    }
                    Node::Object(values) => {
                        output.push('{');
                        pending.push(Task::Text("}"));
                        for (i, (key, &value)) in values.iter().enumerate().rev() {
                            pending.push(Task::Value(value));
                            pending.push(Task::Text(":"));
                            pending.push(Task::Key(key));
                            if i > 0 {
                                pending.push(Task::Text(","));
                            }
                        }
                    }
                },
            }
        }
        Ok(output)
    }
}
