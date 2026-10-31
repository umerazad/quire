//! Opening a finished PDF or Word file before it is saved. The reference only checks the first
//! bytes; Quire also opens the file: a Word file's ZIP and `word/document.xml` must read, and a
//! PDF must open in hayro (memory-safe Rust, as for untrusted PDFs elsewhere) with a page.

use async_zip::base::read::mem::ZipFileReader;
use futures::AsyncReadExt as _;
use quick_xml::events::Event;
use quire_core::ai::export::{Format, has_signature};
use std::collections::HashMap;

/// One paragraph of a Word file: its text and the parts it shows as images, and for tests its
/// style ID.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Paragraph {
    #[cfg(test)]
    pub style: String,
    pub text: String,
    pub images: Vec<String>,
}

/// What a Word file holds: its paragraphs in order, and every part that is not XML, such as
/// images.
#[derive(Clone, Debug, Default)]
pub struct Docx {
    pub paragraphs: Vec<Paragraph>,
    pub media: HashMap<String, Vec<u8>>,
}

impl Docx {
    /// Why this Word file cannot be a page export: no text at all, or an image it shows that
    /// is not in the file, which Word reports as unreadable content.
    pub(super) fn problem(&self) -> Option<String> {
        if self
            .paragraphs
            .iter()
            .all(|paragraph| paragraph.text.trim().is_empty())
        {
            return Some("it holds no text".into());
        }
        self.paragraphs
            .iter()
            .flat_map(|paragraph| &paragraph.images)
            .find(|image| !self.media.contains_key(*image))
            .map(|image| format!("it shows an image, {image}, that is not in the file"))
    }
}

async fn entry(zip: &ZipFileReader, index: usize) -> Result<Vec<u8>, String> {
    let mut data = Vec::new();
    zip.reader_with_entry(index)
        .await
        .map_err(|error| error.to_string())?
        .read_to_end(&mut data)
        .await
        .map_err(|error| error.to_string())?;
    Ok(data)
}

fn attribute(element: &quick_xml::events::BytesStart, name: &str) -> Option<String> {
    element
        .try_get_attribute(name)
        .ok()
        .flatten()
        .and_then(|value| {
            value
                .normalized_value(quick_xml::XmlVersion::Implicit1_0)
                .ok()
                .map(|value| value.into_owned())
        })
}

fn predefined(name: &str) -> Option<char> {
    Some(match name {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        _ => return None,
    })
}

/// `word/_rels/document.xml.rels`: relationship ID to the part it names inside the ZIP, or None
/// for a link outside the file.
fn relationships(xml: &str) -> Result<HashMap<String, Option<String>>, String> {
    let mut reader = quick_xml::Reader::from_str(xml);
    let mut found = HashMap::new();
    loop {
        match reader.read_event().map_err(|error| error.to_string())? {
            Event::Start(element) | Event::Empty(element)
                if element.local_name().as_ref() == b"Relationship" =>
            {
                if let (Some(id), Some(target)) =
                    (attribute(&element, "Id"), attribute(&element, "Target"))
                {
                    let external = attribute(&element, "TargetMode")
                        .is_some_and(|mode| mode.eq_ignore_ascii_case("External"));
                    let part = match target.strip_prefix('/') {
                        Some(absolute) => absolute.to_owned(),
                        None => format!("word/{target}"),
                    };
                    found.insert(id, (!external).then_some(part));
                }
            }
            Event::Eof => return Ok(found),
            _ => {}
        }
    }
}

fn paragraphs(
    xml: &str,
    links: &HashMap<String, Option<String>>,
) -> Result<Vec<Paragraph>, String> {
    let mut reader = quick_xml::Reader::from_str(xml);
    let mut found = Vec::new();
    let mut current: Option<Paragraph> = None;
    let mut in_text = false;
    loop {
        match reader.read_event().map_err(|error| error.to_string())? {
            Event::Start(element) | Event::Empty(element) => match element.name().as_ref() {
                b"w:p" => current = Some(Paragraph::default()),
                #[cfg(test)]
                b"w:pStyle" => {
                    if let (Some(paragraph), Some(style)) =
                        (current.as_mut(), attribute(&element, "w:val"))
                    {
                        paragraph.style = style;
                    }
                }
                b"w:t" => in_text = true,
                b"w:tab" => current.iter_mut().for_each(|p| p.text.push('\t')),
                b"a:blip" => {
                    if let (Some(paragraph), Some(id)) =
                        (current.as_mut(), attribute(&element, "r:embed"))
                    {
                        match links.get(&id) {
                            Some(Some(part)) => paragraph.images.push(part.clone()),
                            Some(None) => {}
                            None => paragraph.images.push(id),
                        }
                    }
                }
                _ => {}
            },
            Event::Text(text) if in_text => {
                if let Some(paragraph) = current.as_mut() {
                    paragraph
                        .text
                        .push_str(&text.decode().map_err(|error| error.to_string())?);
                }
            }
            Event::GeneralRef(reference) if in_text => {
                let resolved = match reference.resolve_char_ref() {
                    Ok(Some(character)) => Some(character),
                    _ => reference.decode().ok().and_then(|name| predefined(&name)),
                };
                if let (Some(paragraph), Some(character)) = (current.as_mut(), resolved) {
                    paragraph.text.push(character);
                }
            }
            Event::End(element) => match element.name().as_ref() {
                b"w:t" => in_text = false,
                b"w:p" => found.extend(current.take()),
                _ => {}
            },
            Event::Eof => return Ok(found),
            _ => {}
        }
    }
}

/// Reads a Word file's paragraphs and media, or says why it is not one.
pub async fn read_docx(bytes: Vec<u8>) -> Result<Docx, String> {
    let zip = ZipFileReader::new(bytes)
        .await
        .map_err(|error| format!("it is not a readable ZIP archive ({error})"))?;
    let names: Vec<String> = zip
        .file()
        .entries()
        .iter()
        .map(|entry| entry.filename().as_str().unwrap_or_default().to_owned())
        .collect();
    let index = |name: &str| names.iter().position(|entry| entry == name);
    let document = index("word/document.xml").ok_or("it has no word/document.xml")?;
    let document = String::from_utf8(entry(&zip, document).await?)
        .map_err(|_| "its word/document.xml is not UTF-8 text".to_owned())?;
    let links = match index("word/_rels/document.xml.rels") {
        Some(rels) => relationships(&String::from_utf8_lossy(&entry(&zip, rels).await?))
            .map_err(|error| format!("its relationships do not read: {error}"))?,
        None => HashMap::new(),
    };
    let paragraphs = paragraphs(&document, &links)
        .map_err(|error| format!("its word/document.xml does not read: {error}"))?;
    let mut media = HashMap::new();
    for (position, name) in names.iter().enumerate() {
        if ![".xml", ".rels", "/"].iter().any(|end| name.ends_with(end)) {
            media.insert(name.clone(), entry(&zip, position).await?);
        }
    }
    Ok(Docx { paragraphs, media })
}

/// Opens a finished file of `format` and says why it cannot be saved, if it cannot. Blocks; run
/// it off the UI thread.
pub fn check(format: Format, bytes: &[u8]) -> Result<(), String> {
    if !has_signature(format, bytes) {
        return Err(format!("it does not start as a {} file", format.name()));
    }
    match format {
        Format::Docx => match futures::executor::block_on(read_docx(bytes.to_vec()))?.problem() {
            Some(problem) => Err(problem),
            None => Ok(()),
        },
        Format::Pdf if bytes.len() > quire_media::pdf::MAX_UNTRUSTED_BYTES => Ok(()),
        Format::Pdf => {
            let document = quire_media::pdf::Document::open_untrusted(bytes.to_vec())
                .map_err(|error| format!("it does not open as a PDF ({error})"))?;
            if document.info().pages.is_empty() {
                Err("it opens as a PDF with no pages".into())
            } else {
                Ok(())
            }
        }
    }
}
