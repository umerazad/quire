//! Differential tests against vectors from the frozen rt export and import code
//! (`tools/exchange-vectors.ts`). Each export case answers references, block lookups, page
//! lookups, and titles exactly as the reference notebook did, so these tests check the export
//! rules alone; the native analysis is checked end to end in the app.

use quire_core::exchange::{
    import::{SourceEntry, import_summary, path_key, plan_import, source_entry},
    notebook_export::{ExportReport, export_target, replace_question},
    page_export::{ExportNotebook, FoundBlock, export_markdown, export_name, localize_attachments},
};
use quire_core::markdown::{Reference, ReferenceKind, ReferenceMedia, ReferenceSyntax};
use serde_json::Value;
use std::{cell::RefCell, collections::HashMap, sync::LazyLock};

static VECTORS: LazyLock<Value> = LazyLock::new(|| {
    serde_json::from_str(include_str!(
        "../../../tests/fixtures/exchange-vectors.json"
    ))
    .unwrap()
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

struct Recorded<'a> {
    case: &'a Value,
    parses: HashMap<String, Vec<Reference>>,
    asked: RefCell<Vec<String>>,
}

impl<'a> Recorded<'a> {
    fn new(case: &'a Value) -> Self {
        let parses = case["references"]
            .as_array()
            .unwrap()
            .iter()
            .map(|parse| {
                let text = parse["text"].as_str().unwrap();
                let references = parse["references"]
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
                (text.to_owned(), references)
            })
            .collect();
        Self {
            case,
            parses,
            asked: RefCell::default(),
        }
    }
}

impl ExportNotebook for Recorded<'_> {
    fn text(&self, path: &str) -> String {
        string(&self.case["docs"][path]).unwrap_or_default()
    }

    fn title(&self, path: &str) -> String {
        string(&self.case["titles"][path]).expect("the reference asked for this title")
    }

    fn find_page(&self, target: &str) -> Option<String> {
        let found = &self.case["finds"][target];
        assert!(
            !found.is_null() || self.case["finds"].get(target).is_some(),
            "the reference never looked up {target}"
        );
        string(found)
    }

    fn resolve(&self, target: &str) -> Option<FoundBlock> {
        self.asked.borrow_mut().push(target.to_owned());
        let found = self.case["resolves"]
            .get(target)
            .unwrap_or_else(|| panic!("the reference never resolved {target}"));
        let path = string(&found["path"])?;
        let text = self.text(&path);
        Some(FoundBlock {
            from: byte(&text, found["from"].as_u64().unwrap()),
            content_from: byte(&text, found["contentFrom"].as_u64().unwrap()),
            subtree_to: byte(&text, found["subtreeTo"].as_u64().unwrap()),
            text: string(&found["text"]).unwrap(),
            path,
        })
    }

    fn references(&self, text: &str) -> Vec<Reference> {
        self.parses
            .get(text)
            .unwrap_or_else(|| panic!("the reference never parsed {text:?}"))
            .clone()
    }
}

#[test]
fn pages_export_as_the_reference_flattens_them() {
    for case in VECTORS["exports"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let notebook = Recorded::new(case);
        let result = export_markdown(&notebook, case["path"].as_str().unwrap());
        if case["error"].as_bool().unwrap() {
            let error = result.expect_err(name);
            assert!(
                error.to_string().contains("Fix the link"),
                "{name}: {error}"
            );
        } else {
            assert_eq!(result.as_deref().ok(), case["output"].as_str(), "{name}");
        }
    }
}

#[test]
fn attachments_localize_and_names_clean_as_the_reference_does() {
    for case in VECTORS["localized"].as_array().unwrap() {
        let mut asked = Vec::new();
        let output = localize_attachments(case["text"].as_str().unwrap(), |relative| {
            asked.push(relative.to_owned());
            match relative {
                "img/chart.png" => Some("/notes/img/chart.png".into()),
                "a/b c.pdf" => Some("/notes/a/b c.pdf".into()),
                _ => None,
            }
        });
        assert_eq!(output, case["output"].as_str().unwrap());
        assert_eq!(
            asked,
            case["asked"]
                .as_array()
                .unwrap()
                .iter()
                .map(|value| value.as_str().unwrap())
                .collect::<Vec<_>>()
        );
    }
    for case in VECTORS["names"].as_array().unwrap() {
        assert_eq!(
            export_name(case["title"].as_str().unwrap(), "md"),
            case["name"].as_str().unwrap()
        );
    }
}

/// Walks a vector source tree in listed order, as the reference does.
fn walk(entries: &Value, prefix: &str, pages: &mut Vec<String>, files: &mut Vec<String>) {
    for entry in entries.as_array().unwrap() {
        let name = entry["name"].as_str().unwrap();
        let path = if prefix.is_empty() {
            name.to_owned()
        } else {
            format!("{prefix}/{name}")
        };
        match source_entry(name, entry.get("children").is_some(), entry["file"] == true) {
            SourceEntry::Skipped => {}
            SourceEntry::Folder => walk(&entry["children"], &path, pages, files),
            SourceEntry::Page => pages.push(path),
            SourceEntry::File => files.push(path),
        }
    }
}

#[test]
fn imports_plan_as_the_reference_does() {
    for case in VECTORS["imports"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let (mut pages, mut files) = (Vec::new(), Vec::new());
        walk(&case["source"], "", &mut pages, &mut files);
        let taken: Vec<String> = case["existing"]
            .as_array()
            .unwrap()
            .iter()
            .map(|path| path_key(path.as_str().unwrap()))
            .collect();
        let plan = plan_import(&pages, |key| taken.iter().any(|taken| taken == key));
        if case["error"].as_bool().unwrap() {
            let error = plan.expect_err(name).to_string();
            assert!(error.starts_with("Cannot import CON.md:"), "{error}");
            assert!(error.ends_with("nothing was imported."), "{error}");
            continue;
        }
        let plan = plan.unwrap();
        let result = &case["result"];
        let strings = |value: &Value| {
            value
                .as_array()
                .unwrap()
                .iter()
                .map(|item| item.as_str().unwrap().to_owned())
                .collect::<Vec<_>>()
        };
        assert_eq!(pages, strings(&result["sources"]), "{name}");
        assert_eq!(files, strings(&result["files"]), "{name}");
        assert_eq!(
            plan.pages
                .iter()
                .map(|(_, target)| target.clone())
                .collect::<Vec<_>>(),
            strings(&result["written"]),
            "{name}"
        );
        assert_eq!(
            plan.skipped.len() as u64,
            result["skipped"].as_u64().unwrap(),
            "{name}"
        );
    }
}

#[test]
fn import_summaries_match_the_reference_wording() {
    assert_eq!(
        import_summary(1, 1, 1),
        "Imported 1 pages and 1 other files. 1 pages were already here and were left unchanged."
    );
    assert_eq!(
        import_summary(3, 0, 0),
        "Imported 3 pages and 0 other files."
    );
}

#[test]
fn notebook_exports_refuse_escaping_paths_and_report_what_stays_behind() {
    assert_eq!(
        export_target("Projects/Plan.md").unwrap(),
        "Projects/Plan.md"
    );
    for bad in [
        "../x.md",
        "a/../../x.md",
        "/etc/passwd",
        "a//b.md",
        "CON.md",
    ] {
        let error = export_target(bad).unwrap_err();
        assert!(
            error.starts_with(&format!("Cannot export {bad}:"))
                && error.ends_with("nothing was written."),
            "{error}"
        );
    }
    let report = ExportReport {
        folder: "/out".into(),
        pages: 1,
        files: 2,
        competing: vec!["A.md".into()],
        drafts: (1..=7).map(|n| format!("D{n}.md")).collect(),
        uncopied: vec!["attachments/x.png".into()],
    };
    assert_eq!(
        report.summary(),
        "Exported 1 page and 2 other files to /out. Not exported: page history and earlier versions; other competing versions of 1 page (A.md; the shown version was exported); unsaved recovery drafts of 7 pages (D1.md, D2.md, D3.md, D4.md, D5.md, and 2 more); 1 attachment the pages link to (attachments/x.png); sync logs, notebook settings, and this computer's records."
    );
    let (question, detail) = replace_question("/out", &["A.md".into(), "B.md".into()]);
    assert_eq!(question, "Replace 2 files in /out?");
    assert_eq!(
        detail,
        "2 files already exist: A.md, B.md. Replacing them cannot be undone. Choose Cancel to pick an empty folder instead."
    );
}
