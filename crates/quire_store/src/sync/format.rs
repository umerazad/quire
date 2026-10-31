use super::json::{JsString, Json, ValueRef};
use sha2::{Digest, Sha256};
use std::{collections::HashSet, fmt};
use unicode_normalization::UnicodeNormalization;

pub const FORMAT: u32 = 1;
pub const MAX_BATCH_BYTES: usize = 256 * 1024 * 1024;
const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Batch,
    Checkpoint,
}
impl Kind {
    fn suffix(self) -> &'static str {
        match self {
            Self::Batch => ".qlog.zst",
            Self::Checkpoint => ".qsnap.zst",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Expected<'a> {
    pub notebook: &'a str,
    pub device: &'a str,
}
#[derive(Debug, PartialEq, Eq)]
pub struct ParsedName<'a> {
    pub seq: u32,
    pub hash: &'a str,
}
#[derive(Debug)]
pub struct Encoded {
    pub name: String,
    pub hash: String,
    pub data: Vec<u8>,
}

/// Retains unknown fields and UTF-16 strings without converting editor text.
#[derive(Debug)]
pub struct Decoded {
    pub value: Json,
    pub hash: String,
}

#[derive(Debug, PartialEq)]
pub enum Error {
    Damaged(String),
    NewerFormat(f64),
    Encode(String),
}
impl Error {
    pub fn reason(&self) -> String {
        match self {
            Self::Damaged(reason) | Self::Encode(reason) => reason.clone(),
            Self::NewerFormat(version) => {
                let mut buffer = ryu_js::Buffer::new();
                let version = if *version == f64::INFINITY {
                    "Infinity"
                } else {
                    buffer.format(*version)
                };
                format!("the file uses format {version}; update Quire to read it")
            }
        }
    }
    pub fn newer_format(&self) -> Option<f64> {
        match self {
            Self::NewerFormat(version) => Some(*version),
            _ => None,
        }
    }
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NewerFormat(_) => f.write_str(&self.reason()),
            Self::Damaged(_) => write!(
                f,
                "Cannot read change record: {}. Preserve the file and retry after sync finishes.",
                self.reason()
            ),
            Self::Encode(_) => write!(
                f,
                "Cannot encode change record: {}. Correct the record before publishing.",
                self.reason()
            ),
        }
    }
}
impl std::error::Error for Error {}

pub fn canonical(value: impl Into<Json>) -> Result<String, Error> {
    canonical_ref(value.into().root())
}
fn canonical_ref(value: ValueRef<'_>) -> Result<String, Error> {
    value
        .canonical()
        .map_err(|error| Error::Encode(error.to_string()))
}
pub fn sha256(data: impl AsRef<[u8]>) -> String {
    format!("{:x}", Sha256::digest(data.as_ref()))
}
pub fn page_id_for_path(path: &str) -> String {
    let normalized = path.nfc().collect::<String>().to_lowercase();
    format!("p-{}", &sha256(normalized)[..32])
}

pub fn revision_id(fields: impl Into<Json>) -> Result<String, Error> {
    revision_id_ref(fields.into().root())
}
fn revision_id_ref(fields: ValueRef<'_>) -> Result<String, Error> {
    revision_content_problem(fields).map_err(damaged)?;
    let mut parents: Vec<_> = fields
        .field("parents")
        .elements()
        .unwrap()
        .map(|id| id.as_str().unwrap())
        .collect();
    parents.sort_unstable();
    let content = format!(
        "{{\"deleted\":{},\"format\":{FORMAT},\"generation\":{},\"page\":{},\"parents\":{},\"path\":{},\"text\":{}}}",
        canonical_ref(fields.field("deleted"))?,
        canonical_ref(fields.field("generation"))?,
        canonical_ref(fields.field("page"))?,
        serde_json::to_string(&parents).unwrap(),
        canonical_ref(fields.field("path"))?,
        canonical_ref(fields.field("text"))?,
    );
    Ok(sha256(content))
}

pub fn validate_revision(value: impl Into<Json>) -> Result<(), Error> {
    validate_revision_ref(value.into().root())
}
pub(crate) fn validate_revision_ref(value: ValueRef<'_>) -> Result<(), Error> {
    revision_content_problem(value).map_err(damaged)?;
    if !is_count(value.field("created")) || !value.field("device").is_string() {
        return Err(damaged("a revision has an invalid origin"));
    }
    if value.field("id").as_str() != Some(revision_id_ref(value)?.as_str()) {
        return Err(damaged("a revision's id does not match its content"));
    }
    Ok(())
}
fn revision_content_problem(value: ValueRef<'_>) -> Result<(), &'static str> {
    if !value.is_object() && !value.is_array() {
        return Err("a revision is not an object");
    }
    if value
        .field("page")
        .as_js_string()
        .is_none_or(JsString::is_empty)
    {
        return Err("a revision has no page");
    }
    let Some(mut parents) = value.field("parents").elements() else {
        return Err("a revision has invalid parents");
    };
    let mut seen = HashSet::new();
    if parents.len() > 16
        || !parents.all(|id| {
            id.as_str()
                .is_some_and(|id| is_hex64(id) && seen.insert(id))
        })
    {
        return Err("a revision has invalid parents");
    }
    if !is_count(value.field("generation")) {
        return Err("a revision has an invalid generation");
    }
    if !value.field("path").is_string()
        || !value.field("text").is_string()
        || !value.field("deleted").is_boolean()
    {
        return Err("a revision has invalid content");
    }
    Ok(())
}
fn is_count(value: ValueRef<'_>) -> bool {
    value
        .as_f64()
        .is_some_and(|n| (0.0..=MAX_SAFE_INTEGER).contains(&n) && n.fract() == 0.0)
}
fn is_hex64(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn damaged(reason: impl Into<String>) -> Error {
    Error::Damaged(reason.into())
}

pub fn parse_name(name: &str, kind: Kind) -> Option<ParsedName<'_>> {
    let stem = name.strip_suffix(kind.suffix())?;
    let (seq, hash) = stem.split_once('-')?;
    if seq.len() != 9 || !seq.bytes().all(|b| b.is_ascii_digit()) || !is_hex64(hash) {
        return None;
    }
    Some(ParsedName {
        seq: seq.parse().ok()?,
        hash,
    })
}
pub fn attachment_hash(name: &str) -> Option<&str> {
    let (hash, extension) = name
        .split_once('.')
        .map_or((name, None), |(h, e)| (h, Some(e)));
    if !is_hex64(hash)
        || extension.is_some_and(|ext| {
            ext.is_empty()
                || ext.len() > 10
                || !ext
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        })
    {
        return None;
    }
    Some(hash)
}
pub fn attachment_file_name(data: &[u8], original: &str) -> String {
    attachment_name_for_hash(&sha256(data), original)
}

/// The protocol name for content with this SHA-256, keeping a short alphanumeric extension.
pub fn attachment_name_for_hash(hash: &str, original: &str) -> String {
    let extension = original.rsplit_once('.').map(|(_, ext)| ext).filter(|ext| {
        !ext.is_empty() && ext.len() <= 10 && ext.bytes().all(|b| b.is_ascii_alphanumeric())
    });
    match extension {
        Some(ext) => format!("{hash}.{}", ext.to_ascii_lowercase()),
        None => hash.to_owned(),
    }
}

pub fn validate_status(value: impl Into<Json>) -> Result<(), Error> {
    let json = value.into();
    let value = json.root();
    if !value.is_object() && !value.is_array() {
        return Err(damaged("status is not an object"));
    }
    if ["format", "lastSeq", "outbox", "heartbeat"]
        .iter()
        .all(|key| is_count(value.field(key)))
        && ["app", "device", "label"]
            .iter()
            .all(|key| value.field(key).is_string())
    {
        Ok(())
    } else {
        Err(damaged("status is incomplete"))
    }
}

fn validate(
    value: ValueRef<'_>,
    kind: Kind,
    expected: Expected<'_>,
    seq: u32,
) -> Result<(), Error> {
    if value
        .field("format")
        .as_f64()
        .is_some_and(|v| v > f64::from(FORMAT))
    {
        return Err(Error::NewerFormat(value.field("format").as_f64().unwrap()));
    }
    if value.field("format").as_f64() != Some(f64::from(FORMAT)) {
        return Err(damaged("the file has an unknown format"));
    }
    for (valid, reason) in [
        (
            value.field("notebook").as_str() == Some(expected.notebook),
            "the file belongs to another notebook",
        ),
        (
            value.field("device").as_str() == Some(expected.device),
            "the file is in another computer's folder",
        ),
        (
            value.field("seq").as_f64() == Some(f64::from(seq)),
            "the file's number does not match its name",
        ),
        (
            is_count(value.field("created")),
            "the file has no creation time",
        ),
    ] {
        if !valid {
            return Err(damaged(reason));
        }
    }
    let revisions = match kind {
        Kind::Batch => {
            let previous = value.get("previous");
            if !previous.is_some_and(|v| v.is_null() || v.as_str().is_some_and(is_hex64))
                || (seq == 1) != previous.is_some_and(ValueRef::is_null)
            {
                return Err(damaged("the file's link to the previous one is invalid"));
            }
            if !value
                .field("attachments")
                .elements()
                .is_some_and(|mut items| {
                    items.all(|v| v.as_str().and_then(attachment_hash).is_some())
                })
            {
                return Err(damaged("the file lists invalid attachments"));
            }
            value
                .field("revisions")
                .elements()
                .ok_or_else(|| damaged("the file has no revisions"))?
        }
        Kind::Checkpoint => {
            if !value.field("cursors").members().is_some_and(|mut cursors| {
                cursors.all(|(_, cursor)| {
                    cursor.is_object()
                        && is_count(cursor.field("seq"))
                        && cursor.field("hash").as_str().is_some_and(is_hex64)
                })
            }) {
                return Err(damaged("the checkpoint's positions are invalid"));
            }
            value
                .field("heads")
                .elements()
                .ok_or_else(|| damaged("the checkpoint has no pages"))?
        }
    };
    for revision in revisions {
        validate_revision_ref(revision)?;
        if kind == Kind::Batch && revision.field("device").as_str() != Some(expected.device) {
            return Err(damaged("a revision names another computer"));
        }
    }
    Ok(())
}

pub fn decode(
    name: &str,
    data: &[u8],
    kind: Kind,
    expected: Expected<'_>,
) -> Result<Decoded, Error> {
    decode_with_limit(name, data, kind, expected, MAX_BATCH_BYTES)
}
fn decode_with_limit(
    name: &str,
    data: &[u8],
    kind: Kind,
    expected: Expected<'_>,
    limit: usize,
) -> Result<Decoded, Error> {
    let parsed = parse_name(name, kind).ok_or_else(|| {
        damaged(match kind {
            Kind::Batch => "the file name is not a batch",
            Kind::Checkpoint => "the file name is not a checkpoint",
        })
    })?;
    let incomplete = || damaged("the file is incomplete or not compressed");
    let text = crate::compression::decompress(data, limit).map_err(|_| incomplete())?;
    if sha256(&text) != parsed.hash {
        return Err(damaged("the file's content does not match its name"));
    }
    let value = Json::parse(&text).map_err(|_| damaged("the file is not valid JSON"))?;
    if !value.root().is_object() {
        return Err(damaged("the file does not hold a record"));
    }
    validate(value.root(), kind, expected, parsed.seq)?;
    Ok(Decoded {
        value,
        hash: parsed.hash.into(),
    })
}

/// Validates before writing so locally produced records are readable by peers.
pub fn encode(value: impl Into<Json>, kind: Kind) -> Result<Encoded, Error> {
    let fail = |reason: &str| Error::Encode(reason.into());
    let mut value = value.into();
    if !value.set_number("format", f64::from(FORMAT)) {
        return Err(fail("the value is not a record"));
    }
    let root = value.root();
    let seq = root
        .field("seq")
        .as_f64()
        .filter(|v| v.fract() == 0.0 && (0.0..=999_999_999.0).contains(v))
        .ok_or_else(|| fail("the sequence does not fit the nine-digit file name"))?
        as u32;
    let expected = Expected {
        notebook: root
            .field("notebook")
            .as_str()
            .ok_or_else(|| fail("the notebook is missing"))?,
        device: root
            .field("device")
            .as_str()
            .ok_or_else(|| fail("the device is missing"))?,
    };
    validate(root, kind, expected, seq).map_err(|error| Error::Encode(error.reason()))?;
    let text = canonical_ref(value.root())?;
    if text.len() > MAX_BATCH_BYTES {
        return Err(fail("the record exceeds the decompressed size limit"));
    }
    let hash = sha256(&text);
    let data = zstd::stream::encode_all(text.as_bytes(), 0)
        .map_err(|error| fail(&format!("zstd compression failed: {error}")))?;
    Ok(Encoded {
        name: format!("{seq:09}-{hash}{}", kind.suffix()),
        hash,
        data,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn decompression_stops_at_the_limit() {
        let text = vec![b' '; 8192];
        let data = zstd::stream::encode_all(text.as_slice(), 0).unwrap();
        let name = format!("000000001-{}.qlog.zst", sha256(&text));
        let error = decode_with_limit(
            &name,
            &data,
            Kind::Batch,
            Expected {
                notebook: "n",
                device: "d",
            },
            1024,
        )
        .unwrap_err();
        assert_eq!(error.reason(), "the file is incomplete or not compressed");
    }
}
