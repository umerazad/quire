//! The fake CLI's document tool: turns the `page.md` in its working folder into a small but real
//! Word or PDF file, as an export run's document tools would. Headings become Heading 1 to 3,
//! list items List Bullet, other lines Normal; images given by local path are embedded in Word
//! files. PDFs carry one line of Helvetica per paragraph.

use async_zip::{Compression, ZipEntryBuilder, base::write::ZipFileWriter};
use std::path::Path;

struct Block {
    style: &'static str,
    text: String,
    image: Option<std::path::PathBuf>,
}

/// The local file an image link `![alt](<path>)` or `![alt](path)` names, and the line without it.
fn image_of(line: &str) -> (String, Option<std::path::PathBuf>) {
    let Some(start) = line.find("![") else {
        return (line.to_owned(), None);
    };
    let Some(open) = line[start..].find("](").map(|at| start + at + 2) else {
        return (line.to_owned(), None);
    };
    let (target, rest) = if line[open..].starts_with('<') {
        match line[open + 1..].find(">)") {
            Some(end) => (&line[open + 1..open + 1 + end], &line[open + end + 3..]),
            None => return (line.to_owned(), None),
        }
    } else {
        match line[open..].find(')') {
            Some(end) => (&line[open..open + end], &line[open + end + 1..]),
            None => return (line.to_owned(), None),
        }
    };
    let path = Path::new(target);
    let image = (path.is_absolute() && path.is_file()).then(|| path.to_owned());
    (format!("{}{}", &line[..start], rest), image)
}

fn blocks(page: &str) -> Vec<Block> {
    page.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let trimmed = line.trim_start();
            let (style, text) = match trimmed {
                _ if trimmed.starts_with("### ") => ("Heading3", &trimmed[4..]),
                _ if trimmed.starts_with("## ") => ("Heading2", &trimmed[3..]),
                _ if trimmed.starts_with("# ") => ("Heading1", &trimmed[2..]),
                _ if trimmed.starts_with("- ") => ("ListBullet", &trimmed[2..]),
                _ => ("Normal", trimmed),
            };
            let (text, image) = image_of(text);
            Block {
                style,
                text: text.trim().to_owned(),
                image,
            }
        })
        .collect()
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

const CONTENT_TYPES: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Default Extension="png" ContentType="image/png"/><Default Extension="jpg" ContentType="image/jpeg"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>"#;
const PACKAGE_RELS: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#;

fn drawing(id: usize, relationship: &str) -> String {
    format!(
        r#"<w:r><w:drawing><wp:inline><wp:extent cx="914400" cy="914400"/><wp:docPr id="{id}" name="Image {id}"/><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/picture"><pic:pic><pic:nvPicPr><pic:cNvPr id="{id}" name="Image {id}"/><pic:cNvPicPr/></pic:nvPicPr><pic:blipFill><a:blip r:embed="{relationship}"/></pic:blipFill><pic:spPr/></pic:pic></a:graphicData></a:graphic></wp:inline></w:drawing></w:r>"#
    )
}

async fn docx(blocks: &[Block]) -> Vec<u8> {
    let mut body = String::new();
    let mut rels = String::new();
    let mut media = Vec::new();
    for block in blocks {
        body.push_str(&format!(
            r#"<w:p><w:pPr><w:pStyle w:val="{}"/></w:pPr>"#,
            block.style
        ));
        if !block.text.is_empty() {
            body.push_str(&format!(
                r#"<w:r><w:t xml:space="preserve">{}</w:t></w:r>"#,
                escape(&block.text)
            ));
        }
        if let Some(image) = &block.image {
            let number = media.len() + 1;
            let extension = image
                .extension()
                .map(|extension| extension.to_string_lossy().into_owned())
                .unwrap_or_else(|| "png".into());
            let name = format!("media/image{number}.{extension}");
            let relationship = format!("rIdImage{number}");
            rels.push_str(&format!(
                r#"<Relationship Id="{relationship}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="{name}"/>"#
            ));
            body.push_str(&drawing(number, &relationship));
            media.push((
                format!("word/{name}"),
                std::fs::read(image).expect("the image named in page.md cannot be read"),
            ));
        }
        body.push_str("</w:p>");
    }
    let document = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:pic="http://schemas.openxmlformats.org/drawingml/2006/picture"><w:body>{body}</w:body></w:document>"#
    );
    let document_rels = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">{rels}</Relationships>"#
    );
    let mut out = futures::io::Cursor::new(Vec::new());
    let mut zip = ZipFileWriter::new(&mut out);
    let parts = [
        (
            "[Content_Types].xml".to_owned(),
            CONTENT_TYPES.as_bytes().to_vec(),
        ),
        ("_rels/.rels".to_owned(), PACKAGE_RELS.as_bytes().to_vec()),
        ("word/document.xml".to_owned(), document.into_bytes()),
        (
            "word/_rels/document.xml.rels".to_owned(),
            document_rels.into_bytes(),
        ),
    ];
    for (name, data) in parts.into_iter().chain(media) {
        zip.write_entry_whole(
            ZipEntryBuilder::new(name.into(), Compression::Deflate),
            &data,
        )
        .await
        .expect("the fake Word file could not be written");
    }
    zip.close()
        .await
        .expect("the fake Word file could not be finished");
    out.into_inner()
}

/// A PDF with one line of Helvetica per paragraph, 40 lines to a US Letter page.
fn pdf(blocks: &[Block]) -> Vec<u8> {
    let lines: Vec<String> = blocks
        .iter()
        .filter(|block| !block.text.is_empty())
        .map(|block| {
            block
                .text
                .chars()
                .filter(char::is_ascii)
                .collect::<String>()
                .replace('\\', "\\\\")
                .replace('(', "\\(")
                .replace(')', "\\)")
        })
        .collect();
    let pages: Vec<&[String]> = if lines.is_empty() {
        vec![&[]]
    } else {
        lines.chunks(40).collect()
    };
    let count = pages.len();
    let mut objects = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        format!(
            "<< /Type /Pages /Kids [{}] /Count {count} >>",
            (0..count)
                .map(|page| format!("{} 0 R", 4 + page * 2))
                .collect::<Vec<_>>()
                .join(" ")
        ),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".into(),
    ];
    for (page, lines) in pages.iter().enumerate() {
        let stream = lines
            .iter()
            .enumerate()
            .map(|(row, line)| format!("BT /F1 12 Tf 72 {} Td ({line}) Tj ET", 720 - row * 16))
            .collect::<Vec<_>>()
            .join("\n");
        objects.push(format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 3 0 R >> >> /Contents {} 0 R >>",
            5 + page * 2
        ));
        objects.push(format!(
            "<< /Length {} >>\nstream\n{stream}\nendstream",
            stream.len()
        ));
    }
    let mut out = String::from("%PDF-1.4\n");
    let mut offsets = Vec::new();
    for (index, object) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.push_str(&format!("{} 0 obj\n{object}\nendobj\n", index + 1));
    }
    let xref = out.len();
    out.push_str(&format!(
        "xref\n0 {}\n0000000000 65535 f \n",
        objects.len() + 1
    ));
    for offset in offsets {
        out.push_str(&format!("{offset:010} 00000 n \n"));
    }
    out.push_str(&format!(
        "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
        objects.len() + 1
    ));
    out.into_bytes()
}

/// Writes `name` (`export.docx` or `export.pdf`) from `page.md` in the working folder.
pub fn render(name: &str) {
    let page =
        std::fs::read_to_string("page.md").expect("page.md is missing from the working folder");
    let blocks = blocks(&page);
    let bytes = if name.ends_with(".docx") {
        futures::executor::block_on(docx(&blocks))
    } else {
        pdf(&blocks)
    };
    std::fs::write(name, bytes).expect("the fake CLI could not write its document");
}
