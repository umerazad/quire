use quire_core::{
    markdown::{page_title, references},
    relocate::{
        CASE_ONLY, FOLDER_EMPTY, FOLDER_INSIDE_ITSELF, FOLDER_UNCHANGED, NAME_HAS_SLASH,
        Relocation, folder_relocations, notebook_path, relative_path, relocate_page,
        relocate_pages, rename_target,
    },
    source_text::SourceText,
};
use serde_json::Value;
use std::collections::BTreeMap;

fn ranges(value: &Value) -> Vec<std::ops::Range<usize>> {
    value
        .as_array()
        .unwrap()
        .iter()
        .map(|pair| pair[0].as_u64().unwrap() as usize..pair[1].as_u64().unwrap() as usize)
        .collect()
}

/// Plans one vector through the port: every changed page's text, and the listed link changes.
type Planned = (String, BTreeMap<String, String>, Vec<Value>);

fn plan(vector: &Value) -> Result<Planned, String> {
    let pages = vector["pages"].as_array().unwrap();
    let source = vector["source"].as_str().unwrap();
    let paths: Vec<&str> = pages
        .iter()
        .map(|page| page["path"].as_str().unwrap())
        .collect();
    let relocation = Relocation::new(
        source,
        vector["target"].as_str().unwrap(),
        vector["rename"].as_bool().unwrap(),
        vector["sourceTitle"].as_str().unwrap(),
        vector["uniqueTitle"].as_bool().unwrap(),
        paths.contains(&source),
        |target| {
            paths
                .iter()
                .any(|path| path.to_lowercase() == target.to_lowercase())
        },
    )
    .map_err(str::to_owned)?;
    let resolutions = vector["resolutions"].as_object().unwrap();
    let find = |base: &str| {
        resolutions
            .get(base)
            .unwrap_or_else(|| panic!("{}: no resolution for {base:?}", vector["name"]))
            .as_str()
            .map(str::to_owned)
    };
    let mut texts = BTreeMap::new();
    let mut updates = Vec::new();
    for page in pages {
        let path = page["path"].as_str().unwrap();
        let text = page["text"].as_str().unwrap();
        let found = references(text, &ranges(&page["codeRanges"]));
        let Some(planned) = relocate_page(
            &relocation,
            path,
            text,
            &found,
            &ranges(&page["urls"]),
            find,
        ) else {
            continue;
        };
        let (original, normalized) = SourceText::prepare(text.to_owned());
        assert_eq!(normalized, text);
        assert_eq!(
            *original.apply(planned.edits.clone()).unwrap(),
            planned.text,
            "{}: edits and text disagree for {path}",
            vector["name"]
        );
        for change in &planned.changes {
            updates.push(
                serde_json::json!({"path": path, "before": change.before, "after": change.after}),
            );
        }
        texts.insert(planned.path, planned.text);
    }
    Ok((relocation.target, texts, updates))
}

#[test]
fn relocations_match_the_frozen_plan_relocation_vectors() {
    let vectors: Vec<Value> = serde_json::from_str(include_str!(
        "../../../tests/fixtures/relocate-vectors.json"
    ))
    .unwrap();
    assert!(vectors.len() > 150);
    let mut failures = Vec::new();
    for vector in &vectors {
        let expected = &vector["result"];
        let actual = match plan(vector) {
            Err(error) => serde_json::json!({ "error": error }),
            Ok((target, texts, updates)) => {
                // The reference lists updates by page in notebook order, references before URLs.
                let order: Vec<&str> = vector["pages"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|page| page["path"].as_str().unwrap())
                    .collect();
                let mut updates = updates;
                updates.sort_by_key(|update| {
                    order
                        .iter()
                        .position(|path| *path == update["path"].as_str().unwrap())
                });
                serde_json::json!({ "target": target, "texts": texts, "updates": updates })
            }
        };
        let normalize = |value: &Value| {
            let mut value = value.clone();
            if let Some(texts) = value.get_mut("texts") {
                let sorted: BTreeMap<String, Value> =
                    serde_json::from_value(texts.clone()).unwrap();
                *texts = serde_json::to_value(sorted).unwrap();
            }
            value
        };
        if normalize(expected) != normalize(&actual) {
            failures.push(format!(
                "{}\n  expected {}\n  actual   {}",
                vector["name"], expected, actual
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} relocation vectors differ:\n{}",
        failures.len(),
        vectors.len(),
        failures.join("\n")
    );
}

#[test]
fn paths_follow_the_reference_notebook_path_and_relative_path() {
    assert_eq!(
        notebook_path(" Archive/New ", true).unwrap(),
        "Archive/New.md"
    );
    assert_eq!(
        notebook_path("Archive/New.MD", true).unwrap(),
        "Archive/New.MD"
    );
    for invalid in ["../Escape", "A//B", "./x", "con", " /x", ""] {
        assert!(notebook_path(invalid, true).is_err(), "{invalid:?}");
    }
    assert_eq!(
        relative_path("Archive/New.md", "assets/a.png"),
        "../assets/a.png"
    );
    assert_eq!(relative_path("A/B.md", "A/C.md"), "C.md");
    assert_eq!(relative_path("A/B.md", "A"), "A");
    assert_eq!(
        quire_core::relocate::INVALID_NAME,
        quire_core::markdown::PageNameError.to_string()
    );
    assert_eq!(
        notebook_path("Notes/con", true),
        Err(quire_core::relocate::INVALID_NAME)
    );
}

#[test]
fn renaming_in_place_keeps_the_folder_and_refuses_slashes() {
    assert_eq!(
        rename_target("Notes/Old.md", " New name ").unwrap(),
        Some("Notes/New name.md".into())
    );
    assert_eq!(rename_target("Old.md", "Old").unwrap(), None);
    assert_eq!(rename_target("Old.md", "  ").unwrap(), None);
    assert_eq!(rename_target("Old.md", "a/b"), Err(NAME_HAS_SLASH));
    assert_eq!(rename_target("Old.md", "a\\b"), Err(NAME_HAS_SLASH));
    let case_only = Relocation::new("Old.md", "old.md", true, "Old", true, true, |_| false);
    assert_eq!(case_only, Err(CASE_ONLY));
}

/// Deviations from the reference, which these cases document.
#[test]
fn new_names_are_literal_and_fragment_links_stay() {
    let relocation = Relocation::new(
        "Notes/Old.md",
        "Notes/Cost $&.md",
        true,
        "Old",
        true,
        true,
        |_| false,
    )
    .unwrap();
    let text = "- [[Old]] and [see](#head)\n";
    let found = references(text, &[]);
    let url = text.find("#head").unwrap();
    let planned = relocate_page(
        &relocation,
        "Notes/Old.md",
        text,
        &found,
        std::slice::from_ref(&(url..url + 5)),
        |base| (base == "Old").then(|| "Notes/Old.md".to_owned()),
    )
    .unwrap();
    assert_eq!(planned.text, "- [[Notes/Cost $&]] and [see](#head)\n");
    assert_eq!(
        page_title("Notes/Cost $&.md", &planned.text).to_lossy_string(),
        "Cost $&"
    );
}

/// Plans one folder vector through the port: every changed page's text by its final path.
fn plan_folder(vector: &Value) -> Result<BTreeMap<String, String>, String> {
    let titles: Vec<(String, String)> = vector["titles"]
        .as_array()
        .unwrap()
        .iter()
        .map(|pair| {
            (
                pair[0].as_str().unwrap().to_owned(),
                pair[1].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    let relocations = folder_relocations(
        vector["folder"].as_str().unwrap(),
        vector["target"].as_str().unwrap(),
        &titles,
        |target| {
            titles
                .iter()
                .any(|(path, _)| path.to_lowercase() == target.to_lowercase())
        },
    )
    .map_err(str::to_owned)?;
    let resolutions = vector["resolutions"].as_object().unwrap();
    let mut texts = BTreeMap::new();
    for page in vector["pages"].as_array().unwrap() {
        let path = page["path"].as_str().unwrap();
        let text = page["text"].as_str().unwrap();
        let found = references(text, &ranges(&page["codeRanges"]));
        let Some(planned) = relocate_pages(
            &relocations,
            path,
            text,
            &found,
            &ranges(&page["urls"]),
            |base| resolutions[base].as_str().map(str::to_owned),
        ) else {
            continue;
        };
        let (original, _) = SourceText::prepare(text.to_owned());
        assert_eq!(
            *original.apply(planned.edits.clone()).unwrap(),
            planned.text,
            "{}: edits and text disagree for {path}",
            vector["name"]
        );
        if planned.path != path || planned.text != text {
            texts.insert(planned.path, planned.text);
        }
    }
    Ok(texts)
}

#[test]
fn folder_moves_match_the_frozen_plan_relocation_applied_page_by_page() {
    let vectors: Vec<Value> = serde_json::from_str(include_str!(
        "../../../tests/fixtures/folder-relocate-vectors.json"
    ))
    .unwrap();
    assert!(vectors.len() > 60);
    let failures: Vec<String> = vectors
        .iter()
        .filter_map(|vector| {
            let actual = match plan_folder(vector) {
                Err(error) => serde_json::json!({ "error": error }),
                Ok(texts) => serde_json::json!({ "texts": texts }),
            };
            let mut expected = vector["result"].clone();
            if let Some(texts) = expected.get_mut("texts") {
                let sorted: BTreeMap<String, Value> =
                    serde_json::from_value(texts.clone()).unwrap();
                *texts = serde_json::to_value(sorted).unwrap();
            }
            (expected != actual).then(|| {
                format!(
                    "{}\n  expected {expected}\n  actual   {actual}",
                    vector["name"]
                )
            })
        })
        .collect();
    assert!(
        failures.is_empty(),
        "{} of {} folder vectors differ:\n{}",
        failures.len(),
        vectors.len(),
        failures.join("\n")
    );
}

#[test]
fn folder_moves_refuse_the_same_folder_a_folder_inside_itself_and_an_empty_folder() {
    let pages = vec![
        ("Projects/A.md".to_owned(), "A".to_owned()),
        ("Projects2/B.md".to_owned(), "B".to_owned()),
    ];
    let none = |_: &str| false;
    assert_eq!(
        folder_relocations("Projects", "projects", &pages, none),
        Err(FOLDER_UNCHANGED)
    );
    assert_eq!(
        folder_relocations("Projects", "Projects/Sub", &pages, none),
        Err(FOLDER_INSIDE_ITSELF)
    );
    assert_eq!(
        folder_relocations("Empty", "Full", &pages, none),
        Err(FOLDER_EMPTY)
    );
    // A sibling folder sharing the name's start is not inside it and does not move.
    let moved = folder_relocations("Projects", "Projects2/Old", &pages, none).unwrap();
    assert_eq!(
        moved
            .iter()
            .map(|relocation| (relocation.source.as_str(), relocation.target.as_str()))
            .collect::<Vec<_>>(),
        [("Projects/A.md", "Projects2/Old/A.md")]
    );
}
