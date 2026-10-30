use super::{JsString, Json, Node};

impl Json {
    /// String(JSON.parse(...)) for metadata. A noncallable own toString causes fallback.
    pub(crate) fn javascript_string(&self) -> Option<JsString> {
        enum Part {
            Value(usize, bool),
            Comma,
        }
        let mut pending = vec![Part::Value(0, false)];
        let mut output = Vec::new();
        while let Some(part) = pending.pop() {
            let (index, array_item) = match part {
                Part::Comma => {
                    output.push(b',');
                    continue;
                }
                Part::Value(index, array_item) => (index, array_item),
            };
            match &self.nodes[index] {
                Node::Null if array_item => {}
                Node::Null => output.extend_from_slice(b"null"),
                Node::Bool(value) => {
                    output.extend_from_slice(if *value { b"true" } else { b"false" })
                }
                Node::Number(value) => {
                    let mut buffer = ryu_js::Buffer::new();
                    let number = if value.is_nan() {
                        "NaN"
                    } else if *value == f64::INFINITY {
                        "Infinity"
                    } else if *value == f64::NEG_INFINITY {
                        "-Infinity"
                    } else if *value == 0.0 {
                        "0"
                    } else {
                        buffer.format_finite(*value)
                    };
                    output.extend_from_slice(number.as_bytes());
                }
                Node::String(value) => output.extend_from_slice(value.as_bytes()),
                Node::Object(fields) => {
                    if fields.contains_key(&JsString::from("toString")) {
                        return None;
                    }
                    output.extend_from_slice(b"[object Object]");
                }
                Node::Array(values) => {
                    for (position, &index) in values.iter().enumerate().rev() {
                        pending.push(Part::Value(index, true));
                        if position > 0 {
                            pending.push(Part::Comma);
                        }
                    }
                }
            }
        }
        Some(JsString(output))
    }
}
