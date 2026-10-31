//! Which Markdown targets name media, and where their bytes come from. A port of the reference
//! `attachments.ts`: `attachmentKind` and `attachmentURL`.

use crate::MediaError;
use base64::Engine as _;
use quire_core::markdown::{decode_uri_component, relative_vault_path};
use std::sync::LazyLock;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MediaKind {
    Image,
    Pdf,
}

/// Where a media target's bytes live.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum MediaSource {
    /// A notebook-relative path, already resolved against the page's folder.
    Notebook(String),
    /// An `http` or `https` URL, fetched without credentials.
    Remote(String),
    /// A `data:` URI with an image or PDF type.
    Data(String),
}

/// A media reference, ready to load.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct MediaTarget {
    pub kind: MediaKind,
    pub source: MediaSource,
    /// The 1-based page a PDF link asks for with `#page=N`.
    pub page: Option<u32>,
    /// What the toolbar and placeholders call it.
    pub name: String,
}

static PDF: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"\.pdf(?:[?#]|$)|^data:application/pdf[;,]").unwrap());
static IMAGE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"\.(?:png|jpe?g|gif|webp|svg|avif)(?:[?#]|$)|^data:image/").unwrap()
});
static DATA: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"^data:(?:image/(?:png|jpeg|gif|webp|avif|svg\+xml)|application/pdf)[;,]")
        .unwrap()
});
static SCHEME: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"^[a-z][a-z0-9+.-]*:").unwrap());

/// Whether a target names an image or a PDF, by extension or `data:` type.
pub fn media_kind(target: &str) -> Option<MediaKind> {
    let head = target.get(..32).unwrap_or(target).to_ascii_lowercase();
    if head.starts_with("data:") {
        return if PDF.is_match(&head) {
            Some(MediaKind::Pdf)
        } else {
            IMAGE.is_match(&head).then_some(MediaKind::Image)
        };
    }
    let decoded = decode_uri_component(target)
        .unwrap_or_else(|| target.to_owned())
        .to_ascii_lowercase();
    if PDF.is_match(&decoded) {
        Some(MediaKind::Pdf)
    } else if IMAGE.is_match(&decoded) {
        Some(MediaKind::Image)
    } else {
        None
    }
}

/// Where a target's bytes come from, relative to the page at `from`. Other schemes, `//` hosts,
/// and paths that climb above the notebook are refused.
pub fn media_source(target: &str, from: Option<&str>) -> Result<MediaSource, MediaError> {
    let lower = target.get(..8).unwrap_or(target).to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        return Ok(MediaSource::Remote(target.to_owned()));
    }
    if DATA.is_match(&target.get(..32).unwrap_or(target).to_ascii_lowercase()) {
        return Ok(MediaSource::Data(target.to_owned()));
    }
    let refused = || {
        MediaError::Refused(
            "This attachment path cannot be opened. Use a file inside the notebook or an HTTP(S) URL."
                .into(),
        )
    };
    if SCHEME.is_match(&target.get(..32).unwrap_or(target).to_ascii_lowercase())
        || target.starts_with("//")
    {
        return Err(refused());
    }
    let file = target.split('#').next().unwrap_or_default();
    let file = file.split('?').next().unwrap_or_default();
    relative_vault_path(from, file)
        .filter(|path| !path.is_empty())
        .map(MediaSource::Notebook)
        .ok_or_else(refused)
}

/// The `#page=N` a PDF target asks to open at.
pub fn page_fragment(target: &str) -> Option<u32> {
    let (_, fragment) = target.split_once('#')?;
    fragment
        .split('&')
        .find_map(|part| part.strip_prefix("page="))
        .and_then(|page| page.parse().ok())
        .filter(|page| *page > 0)
}

/// The name shown for a target, as the reference names media.
pub fn display_name(target: &str, label: Option<&str>, kind: MediaKind) -> String {
    if let Some(label) = label.filter(|label| !label.is_empty()) {
        return label.to_owned();
    }
    if target.starts_with("data:") {
        return match kind {
            MediaKind::Pdf => "Embedded PDF",
            MediaKind::Image => "Embedded image",
        }
        .into();
    }
    target
        .rsplit(['/', '\\'])
        .next()
        .and_then(|last| last.split(['?', '#']).next())
        .filter(|name| !name.is_empty())
        .unwrap_or("Attachment")
        .to_owned()
}

/// A media target in a page, when `target` names one. `kind` overrides the extension, as a
/// `{{pdf: ...}}` embed does.
pub fn media_target(
    target: &str,
    label: Option<&str>,
    kind: Option<MediaKind>,
    from: Option<&str>,
) -> Option<Result<MediaTarget, MediaError>> {
    let kind = kind.or_else(|| media_kind(target))?;
    Some(media_source(target, from).map(|source| MediaTarget {
        kind,
        page: page_fragment(target),
        name: display_name(target, label, kind),
        source,
    }))
}

/// The bytes of a `data:` URI, refusing payloads over `limit` before decoding them.
pub fn decode_data(uri: &str, limit: usize) -> Result<Vec<u8>, MediaError> {
    let damaged = || {
        MediaError::Failed(
            "This embedded data URI is damaged. Replace it with the original file or URL.".into(),
        )
    };
    let (header, payload) = uri.split_once(',').ok_or_else(damaged)?;
    if payload.len() / 4 * 3 > limit {
        return Err(too_large(limit));
    }
    let bytes = if header.to_ascii_lowercase().ends_with(";base64") {
        let compact: String = payload
            .chars()
            .filter(|c| !c.is_ascii_whitespace())
            .collect();
        base64::engine::general_purpose::STANDARD
            .decode(compact)
            .map_err(|_| damaged())?
    } else {
        decode_uri_component(payload)
            .ok_or_else(damaged)?
            .into_bytes()
    };
    if bytes.len() > limit {
        return Err(too_large(limit));
    }
    Ok(bytes)
}

pub(crate) fn too_large(limit: usize) -> MediaError {
    MediaError::Refused(format!(
        "This attachment is larger than {} MB, the inline limit. Open it in the system viewer instead.",
        limit / (1024 * 1024)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_local_and_remote_media_while_rejecting_escapes_and_active_schemes() {
        assert_eq!(
            media_source("../assets/paper.pdf#page=3", Some("Folder/Page.md")),
            Ok(MediaSource::Notebook("assets/paper.pdf".into()))
        );
        assert_eq!(page_fragment("../assets/paper.pdf#page=3"), Some(3));
        assert_eq!(
            media_source("https://example.com/image.png?token=value", None),
            Ok(MediaSource::Remote(
                "https://example.com/image.png?token=value".into()
            ))
        );
        for refused in [
            "../../outside.png",
            "javascript:alert(1)",
            "file:///private/file.pdf",
            "//host/share.png",
            "data:text/html,<b>x</b>",
        ] {
            assert!(
                matches!(
                    media_source(refused, Some("Page.md")),
                    Err(MediaError::Refused(_))
                ),
                "{refused}"
            );
        }
        assert_eq!(
            media_kind("https://example.com/a%2Fpaper.pdf?alt=media"),
            Some(MediaKind::Pdf)
        );
        assert_eq!(media_kind("image.webp"), Some(MediaKind::Image));
        assert_eq!(
            media_kind("data:image/png;base64,AAAA"),
            Some(MediaKind::Image)
        );
        assert_eq!(
            media_kind("data:application/pdf;base64,AAAA"),
            Some(MediaKind::Pdf)
        );
        assert_eq!(media_kind("notes.md"), None);
        assert_eq!(media_kind("photo.png.md"), None);
    }

    #[test]
    fn names_follow_labels_data_and_file_names() {
        assert_eq!(
            display_name("a/b/photo.png?x=1", None, MediaKind::Image),
            "photo.png"
        );
        assert_eq!(
            display_name("x.png", Some("Photo"), MediaKind::Image),
            "Photo"
        );
        assert_eq!(
            display_name("data:application/pdf,x", None, MediaKind::Pdf),
            "Embedded PDF"
        );
        assert_eq!(
            display_name("data:image/png,x", Some(""), MediaKind::Image),
            "Embedded image"
        );
        let target = media_target("{{weird}}", None, Some(MediaKind::Pdf), None)
            .unwrap()
            .unwrap();
        assert_eq!(target.source, MediaSource::Notebook("{{weird}}".into()));
    }

    #[test]
    fn data_uris_decode_within_limits() {
        assert_eq!(
            decode_data("data:image/png;base64,aGk=", 16),
            Ok(b"hi".to_vec())
        );
        assert_eq!(
            decode_data("data:image/svg+xml,%3Csvg%3E", 16),
            Ok(b"<svg>".to_vec())
        );
        assert!(matches!(
            decode_data("data:image/png;base64,aGk=", 1),
            Err(MediaError::Refused(_))
        ));
        assert!(matches!(
            decode_data("data:image/png;base64,***", 16),
            Err(MediaError::Failed(_))
        ));
    }
}
