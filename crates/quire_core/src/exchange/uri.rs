//! JavaScript's URI escaping functions, as the reference export code calls them.

pub(crate) use crate::markdown::decode_uri_component;

fn hex(byte: u8) -> Option<u8> {
    char::from(byte)
        .to_digit(16)
        .and_then(|digit| u8::try_from(digit).ok())
}

/// `decodeURI`: like `decodeURIComponent`, but an escape for one of `;/?:@&=+$,#` stays escaped.
/// None where JavaScript throws.
pub(crate) fn decode_uri(input: &str) -> Option<String> {
    let bytes = input.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'%' {
            decoded.push(bytes[index]);
            index += 1;
            continue;
        }
        let byte = hex(*bytes.get(index + 1)?)? * 16 + hex(*bytes.get(index + 2)?)?;
        if b";/?:@&=+$,#".contains(&byte) {
            decoded.extend_from_slice(&bytes[index..index + 3]);
        } else {
            decoded.push(byte);
        }
        index += 3;
    }
    String::from_utf8(decoded).ok()
}

/// `encodeURIComponent`.
pub(crate) fn encode_uri_component(input: &str) -> String {
    input
        .bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&byte) {
                char::from(byte).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_follow_javascript() {
        assert_eq!(
            encode_uri_component("img/a b ü.png"),
            "img%2Fa%20b%20%C3%BC.png"
        );
        assert_eq!(decode_uri("a%20b%2Fc%23"), Some("a b%2Fc%23".into()));
        assert_eq!(decode_uri("%E0%A4%A"), None);
        assert_eq!(decode_uri("%FF"), None);
        assert_eq!(decode_uri("caf%C3%A9"), Some("café".into()));
    }
}
