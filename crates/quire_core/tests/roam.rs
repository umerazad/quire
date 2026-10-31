//! Differential tests against vectors from the frozen rt Roam code (`tools/roam-vectors.ts`):
//! JSON and EDN imports, the EDN reader, and JSON exports. Each case answers references and block
//! parses exactly as the reference parser did and numbers new identifiers as the tool did, so these
//! tests check the conversion rules alone; the native parses are checked end to end in the app.

use quire_core::exchange::{
    edn::{import_roam_edn, read_edn, roam_pages_from_edn},
    js_json::{self, parse, stringify},
    roam::{
        Document, Imported, RoamMarkdown, TextBlock, export_file_text, export_roam_json,
        import_roam_json, reference_texts,
    },
};
use quire_core::markdown::{Reference, ReferenceKind, ReferenceMedia, ReferenceSyntax};
use serde_json::Value;
use std::{cell::RefCell, collections::HashMap, sync::LazyLock};

static VECTORS: LazyLock<Value> = LazyLock::new(|| {
    serde_json::from_str(include_str!("../../../tests/fixtures/roam-vectors.json")).unwrap()
});

/// The byte offset of a UTF-16 offset.
fn byte(text: &str, units: u64) -> usize {
    let mut seen = 0;
    for (index, character) in text.char_indices() {
        if seen == units as usize {
            return index;
        }
        seen += character.len_utf16();
    }
    assert_eq!(seen, units as usize, "offset inside a character");
    text.len()
}

fn string(value: &Value) -> Option<String> {
    value.as_str().map(str::to_owned)
}

/// The reference parses a case recorded, answered by text.
struct Recorded {
    references: HashMap<String, Vec<Reference>>,
    blocks: HashMap<String, Vec<TextBlock>>,
    asked: RefCell<Vec<String>>,
}

impl Recorded {
    fn new(case: &Value) -> Self {
        let references = case["references"]
            .as_array()
            .unwrap()
            .iter()
            .map(|parse| {
                let text = parse["text"].as_str().unwrap();
                let found = parse["references"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|reference| Reference {
                        kind: match reference["kind"].as_str().unwrap() {
                            "page" => ReferenceKind::Page,
                            "block" => ReferenceKind::Block,
                            _ => ReferenceKind::Embed,
                        },
                        syntax: if reference["syntax"] == "wiki" {
                            ReferenceSyntax::Wiki
                        } else {
                            ReferenceSyntax::Roam
                        },
                        range: byte(text, reference["from"].as_u64().unwrap())
                            ..byte(text, reference["to"].as_u64().unwrap()),
                        target: string(&reference["target"]).unwrap(),
                        label: string(&reference["label"]),
                        media: (reference["media"] == "pdf").then_some(ReferenceMedia::Pdf),
                    })
                    .collect();
                (text.to_owned(), found)
            })
            .collect();
        let blocks = case["blocks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|parse| {
                let text = parse["text"].as_str().unwrap();
                let found = parse["blocks"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|block| TextBlock {
                        key: string(&block["key"]).unwrap(),
                        id: string(&block["id"]),
                        parent: string(&block["parent"]),
                        from: byte(text, block["from"].as_u64().unwrap()),
                        subtree_to: byte(text, block["subtreeTo"].as_u64().unwrap()),
                        text: string(&block["text"]).unwrap(),
                    })
                    .collect();
                (text.to_owned(), found)
            })
            .collect();
        Self {
            references,
            blocks,
            asked: RefCell::default(),
        }
    }
}

impl RoamMarkdown for Recorded {
    fn references(&self, text: &str) -> Vec<Reference> {
        self.asked.borrow_mut().push(text.to_owned());
        self.references
            .get(text)
            .unwrap_or_else(|| panic!("the reference never parsed references in {text:?}"))
            .clone()
    }

    fn blocks(&self, text: &str) -> Vec<TextBlock> {
        self.blocks
            .get(text)
            .unwrap_or_else(|| panic!("the reference never parsed blocks in {text:?}"))
            .clone()
    }
}

/// The tool's counted identifiers, starting at `first`.
fn counter(first: u64) -> impl FnMut() -> String {
    let mut next = first;
    move || {
        let id = format!("b-{next:032x}");
        next += 1;
        id
    }
}

fn check_import(name: &str, case: &Value, result: Result<Imported, String>) {
    match string(&case["error"]) {
        Some(error) => assert_eq!(result.err(), Some(error), "{name}"),
        None => {
            let imported = result.unwrap_or_else(|error| panic!("{name}: {error}"));
            let expected = &case["result"];
            let documents: Vec<(String, String)> = expected["documents"]
                .as_array()
                .unwrap()
                .iter()
                .map(|document| {
                    (
                        string(&document["path"]).unwrap(),
                        string(&document["text"]).unwrap(),
                    )
                })
                .collect();
            let actual: Vec<(String, String)> = imported
                .documents
                .iter()
                .map(|document| (document.path.clone(), document.text.clone()))
                .collect();
            assert_eq!(actual, documents, "{name}: documents");
            let report: Vec<String> = expected["report"]
                .as_array()
                .unwrap()
                .iter()
                .map(|line| string(line).unwrap())
                .collect();
            assert_eq!(imported.report, report, "{name}: report");
            assert_eq!(
                stringify(&imported.metadata),
                expected["metadata"].as_str().unwrap(),
                "{name}: metadata"
            );
        }
    }
}

#[test]
fn roam_json_imports_as_the_reference_does() {
    for case in VECTORS["imports"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let markdown = Recorded::new(case);
        let value = parse(case["input"].as_str().unwrap()).unwrap();
        let texts = reference_texts(&value);
        let result = import_roam_json(value, &markdown, &mut counter(1));
        if case["error"].is_null() {
            let mut asked: Vec<String> = markdown
                .asked
                .borrow()
                .iter()
                .filter(|text| text.contains("(("))
                .cloned()
                .collect();
            let mut listed = texts.clone();
            asked.sort();
            asked.dedup();
            listed.sort();
            listed.dedup();
            assert_eq!(listed, asked, "{name}: texts parsed before importing");
        }
        check_import(name, case, result);
    }
}

#[test]
fn roam_edn_imports_as_the_reference_does() {
    for case in VECTORS["edn"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let markdown = Recorded::new(case);
        let result = roam_pages_from_edn(case["input"].as_str().unwrap())
            .and_then(|graph| import_roam_edn(graph, &markdown, &mut counter(1)));
        check_import(name, case, result);
    }
}

#[test]
fn edn_reads_as_the_reference_reads_it() {
    for case in VECTORS["reads"].as_array().unwrap() {
        let input = case["input"].as_str().unwrap();
        let result = read_edn(input).map(|value| stringify(&value.to_json()));
        match string(&case["error"]) {
            Some(error) => assert_eq!(result, Err(error), "{input:?}"),
            None => assert_eq!(result, Ok(string(&case["output"]).unwrap()), "{input:?}"),
        }
    }
}

#[test]
fn roam_json_exports_as_the_reference_does() {
    for case in VECTORS["exports"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let markdown = Recorded::new(case);
        let documents: Vec<Document> = case["documents"]
            .as_array()
            .unwrap()
            .iter()
            .map(|document| Document {
                path: string(&document["path"]).unwrap(),
                text: string(&document["text"]).unwrap(),
            })
            .collect();
        let metadata = case["metadata"]
            .as_str()
            .map(|metadata| js_json::parse(metadata).unwrap());
        let result = export_roam_json(
            &documents,
            metadata.as_ref(),
            &markdown,
            &mut counter(case["firstId"].as_u64().unwrap()),
        );
        match string(&case["error"]) {
            Some(error) => assert_eq!(result.err(), Some(error), "{name}"),
            None => {
                let exported = result.unwrap_or_else(|error| panic!("{name}: {error}"));
                assert_eq!(
                    exported.report,
                    case["result"]["report"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|line| string(line).unwrap())
                        .collect::<Vec<_>>(),
                    "{name}: report"
                );
                assert_eq!(
                    export_file_text(exported.pages),
                    case["result"]["pages"].as_str().unwrap(),
                    "{name}: pages"
                );
            }
        }
    }
}
