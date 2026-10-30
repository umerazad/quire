// Adapted from @lezer/markdown 1.8.1, Copyright (C) 2020 Marijn Haverbeke and others.
// MIT license and pinned source provenance: vendor/lezer-markdown/.

use super::JS_SPACE;
use regex::Regex;
use std::sync::LazyLock;

fn expression(pattern: &str) -> Regex {
    Regex::new(&pattern.replace("SPACE", &JS_SPACE[1..JS_SPACE.len() - 1]))
        .expect("Quire's static autolink grammar is invalid. Fix the pattern and rebuild.")
}

static PREFIX: LazyLock<Regex> = LazyLock::new(|| {
    expression(r"^(?:(www\.)|(https?://)|([A-Za-z0-9_.+-]{1,100}@)|(mailto:|xmpp:))")
});
static DOMAIN: LazyLock<Regex> = LazyLock::new(|| {
    expression(r"^[A-Za-z0-9_-]+(?:\.[A-Za-z0-9_-]+)+(?::[0-9]+)?(?:/[^SPACE<]*)?")
});
static LAST_DOMAIN_WORDS: LazyLock<Regex> =
    LazyLock::new(|| expression(r"[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+(?:\z|[/:])"));
static ENTITY: LazyLock<Regex> =
    LazyLock::new(|| expression(r"&(?:#[0-9]+|#x[a-f0-9]+|[A-Za-z0-9_]+);\z"));
static EMAIL: LazyLock<Regex> =
    LazyLock::new(|| expression(r"^[A-Za-z0-9_.+-]+@[A-Za-z0-9_-]+(?:\.[A-Za-z0-9_.-]+)+"));
static XMPP_RESOURCE: LazyLock<Regex> = LazyLock::new(|| expression(r"^/[A-Za-z0-9@.]+"));
static ANGLE: LazyLock<Regex> = LazyLock::new(|| {
    expression(
        r"^(?:[A-Za-z][A-Za-z0-9_+.-]+:[^SPACE>]+|[A-Za-z0-9.!#$%&'*+/=?^_`{|}~-]+@[A-Za-z0-9](?:[A-Za-z0-9-]{0,61}[A-Za-z0-9])?(?:\.[A-Za-z0-9](?:[A-Za-z0-9-]{0,61}[A-Za-z0-9])?)*)>",
    )
});

fn uri_end(text: &str, from: usize) -> Option<usize> {
    let domain = DOMAIN.find(&text[from..])?.as_str();
    if LAST_DOMAIN_WORDS.find(domain)?.as_str().contains('_') {
        return None;
    }
    let mut end = from + domain.len();
    let mut parentheses = domain.bytes().fold(0isize, |balance, byte| {
        balance + isize::from(byte == b')') - isize::from(byte == b'(')
    });
    loop {
        match text.as_bytes()[end - 1] {
            b'?' | b'!' | b'.' | b',' | b':' | b'*' | b'_' | b'~' => end -= 1,
            b')' if parentheses > 0 => {
                end -= 1;
                parentheses -= 1;
            }
            b';' => {
                if let Some(entity) = ENTITY.find(&text[from..end]) {
                    end = from + entity.start();
                } else {
                    break;
                }
            }
            _ => break,
        }
    }
    Some(end)
}

fn email_end(text: &str, from: usize) -> Option<usize> {
    let email = EMAIL.find(&text[from..])?;
    match email.as_str().as_bytes().last()? {
        b'_' | b'-' => None,
        b'.' => Some(from + email.end() - 1),
        _ => Some(from + email.end()),
    }
}

fn link_bracket_end(text: &str, mut position: usize, end: usize) -> usize {
    while position < end {
        match text.as_bytes()[position] {
            b'[' => {
                let Some(close) = text[position + 1..end].find(']') else {
                    break;
                };
                position += close + 2;
            }
            b']' => break,
            _ => position += 1,
        }
    }
    position
}

/// Returns the end byte of a GFM bare URL/email beginning at an eligible inline position.
/// The caller handles code, HTML, and link precedence. `in_link` trims unmatched brackets
/// inside an open Markdown link. Invalid byte boundaries return None.
pub fn bare_url_end(text: &str, start: usize, in_link: bool) -> Option<usize> {
    let after = text.get(start..)?;
    if start > 0
        && (text.as_bytes()[start - 1].is_ascii_alphanumeric()
            || text.as_bytes()[start - 1] == b'_')
    {
        return None;
    }
    let prefix = PREFIX.captures(after)?;
    let end = if prefix.get(1).is_some() || prefix.get(2).is_some() {
        let end = uri_end(text, start + prefix.get(0).unwrap().end())?;
        if in_link {
            link_bracket_end(text, start, end)
        } else {
            end
        }
    } else if prefix.get(3).is_some() {
        email_end(text, start)?
    } else {
        let mut end = email_end(text, start + prefix.get(0).unwrap().end())?;
        if prefix.get(4).unwrap().as_str() == "xmpp:"
            && let Some(resource) = XMPP_RESOURCE.find(&text[end..])
        {
            end += resource.end();
        }
        end
    };
    Some(end)
}

/// Returns the end byte of an angle autolink beginning at '<', using Quire's frozen dialect.
/// Its whitespace/scheme rules differ from the native grammar's CommonMark token.
pub fn angle_autolink_end(text: &str, start: usize) -> Option<usize> {
    let after = text.get(start..)?.strip_prefix('<')?;
    Some(start + 1 + ANGLE.find(after)?.end())
}
