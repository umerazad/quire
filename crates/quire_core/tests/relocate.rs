use quire_core::{
    markdown::{page_title, references},
    relocate::{
        CASE_ONLY, NAME_HAS_SLASH, Relocation, notebook_path, relative_path, relocate_page,
        rename_target,
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
