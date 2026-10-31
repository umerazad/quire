//! Differential tests against vectors from the frozen rt template, guide, and fuzzy modules
//! (`tools/template-vectors.ts`). The reference parser supplies blocks and references, so these
//! tests check template semantics alone; the native analyzer is checked end to end in the app.

use quire_core::daily::{DailyCalendar, DailyLayout};
use quire_core::fuzzy::{fuzzy_filter, fuzzy_match};
use quire_core::markdown::{Reference, ReferenceKind, ReferenceSyntax};
use quire_core::tasks::DailyNames;
use quire_core::templates::{
    Block, CANCELLED, Clock, Context, Expanded, Parsed, Step, TagUse, catalogue, copy_with_new_ids,
    current, expand, guide, is_templates_page, past_names, placement, preserve_root, template_date,
    templates_in_page,
};
use serde_json::{Value, json};
use std::{collections::HashMap, sync::LazyLock};

/// A placeholder the reference test says no starter or example may leave behind.
static LEFT_OVER: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"\{(cursor|ask|today|date|time|page|weekday)(?-u:\b)").unwrap()
});

static VECTORS: LazyLock<Value> = LazyLock::new(|| {
    serde_json::from_str(include_str!(
        "../../../tests/fixtures/template-vectors.json"
    ))
    .unwrap()
});

/// Converts between UTF-16 offsets in the vectors and byte offsets in Rust.
struct Offsets {
    bytes: Vec<usize>,
    units: Vec<usize>,
}

impl Offsets {
    fn new(text: &str) -> Self {
        let mut bytes = Vec::new();
        let mut units = vec![0; text.len() + 1];
        let mut unit = 0;
        for (byte, character) in text.char_indices() {
            units[byte] = unit;
            for _ in 0..character.len_utf16() {
                bytes.push(byte);
            }
            unit += character.len_utf16();
        }
        bytes.push(text.len());
        units[text.len()] = unit;
        Self { bytes, units }
    }

    fn byte(&self, value: &Value) -> usize {
        self.bytes[value.as_u64().unwrap() as usize]
    }

    fn unit(&self, byte: usize) -> usize {
        self.units[byte]
    }
}

fn parsed(text: &str, value: &Value) -> Parsed {
    let offsets = Offsets::new(text);
    let string = |value: &Value| value.as_str().map(str::to_owned);
    Parsed {
        blocks: value["blocks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|block| Block {
                key: string(&block["key"]).unwrap(),
                id: string(&block["id"]),
                list: block["kind"] == "list",
                from: offsets.byte(&block["from"]),
                content_from: offsets.byte(&block["contentFrom"]),
                content_to: offsets.byte(&block["contentTo"]),
                subtree_to: offsets.byte(&block["subtreeTo"]),
                parent: string(&block["parent"]),
                text: string(&block["text"]).unwrap(),
            })
            .collect(),
        references: value["references"]
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
                range: offsets.byte(&reference["from"])..offsets.byte(&reference["to"]),
                target: string(&reference["target"]).unwrap(),
                label: string(&reference["label"]),
                media: None,
            })
            .collect(),
    }
}

fn days(layout: usize) -> DailyNames {
    let layout = match layout {
        0 => DailyLayout::default(),
        _ => DailyLayout {
            folder: "Daily".into(),
            title: "{MMMM} {DD}, {YYYY}".into(),
        },
    };
    DailyNames::new(layout, DailyCalendar::utc())
}

fn ids() -> impl FnMut() -> String {
    let mut counter = 0;
    move || {
        counter += 1;
        format!("b-00000000000040008000{counter:012x}")
    }
}

const SOURCES: [&str; 2] = ["Templates", "Templates"];

#[test]
fn only_the_root_templates_page_holds_templates() {
    for case in VECTORS["paths"].as_array().unwrap() {
        let path = case["path"].as_str().unwrap();
        let text = "- Meeting\n  - Agenda\n";
        let blocks = vec![
            Block {
                key: "at-0".into(),
                id: None,
                list: true,
                from: 0,
                content_from: 2,
                content_to: 9,
                subtree_to: 20,
                parent: None,
                text: "Meeting".into(),
            },
            Block {
                key: "at-10".into(),
                id: None,
                list: true,
                from: 12,
                content_from: 14,
                content_to: 20,
                subtree_to: 20,
                parent: Some("at-0".into()),
                text: "Agenda".into(),
            },
        ];
        assert_eq!(
            templates_in_page(path, text, &blocks).len() as u64,
            case["definitions"].as_u64().unwrap(),
            "{path}"
        );
        assert_eq!(is_templates_page(path), case["definitions"] == 1, "{path}");
    }
}

#[test]
fn templates_parse_copy_and_expand_as_the_reference_does() {
    let mut checked = 0;
    let examples: String = guide::EXAMPLES.iter().map(guide::definition).collect();
    let starters = guide::starter_templates();
    let offered = [starters.as_str(), examples.as_str()];
    let mut offered_checked = 0;
    for page in VECTORS["pages"].as_array().unwrap() {
        let name = page["name"].as_str().unwrap();
        let text = page["text"].as_str().unwrap();
        let found = templates_in_page("Templates.md", text, &parsed(text, &page["parsed"]).blocks);
        let expected = page["definitions"].as_array().unwrap();
        assert_eq!(
            found
                .iter()
                .map(|definition| json!({"key": definition.key, "name": definition.name, "body": definition.body}))
                .collect::<Vec<_>>(),
            expected
                .iter()
                .map(|definition| json!({"key": definition["key"], "name": definition["name"], "body": definition["body"]}))
                .collect::<Vec<_>>(),
            "{name} definitions"
        );
        let mut copies = HashMap::new();
        for definition in expected {
            let body = definition["body"].as_str().unwrap();
            let body_parsed = parsed(body, &definition["bodyParsed"]);
            for copy in definition["copies"].as_array().unwrap() {
                let destination = copy["destination"].as_str().unwrap();
                let copied = copy_with_new_ids(body, &body_parsed, destination, &SOURCES, ids());
                assert_eq!(copied, copy["text"], "{name} copy into {destination}");
                let copy_parsed = parsed(&copied, &copy["parsed"]);
                copies.insert(
                    (definition["key"].as_str().unwrap(), destination),
                    (copied, copy_parsed),
                );
            }
        }
        for expansion in page["expansions"].as_array().unwrap() {
            let key = expansion["key"].as_str().unwrap();
            let destination = expansion["destination"].as_str().unwrap();
            let (copied, copy_parsed) = &copies[&(key, destination)];
            let days = days(expansion["layout"].as_u64().unwrap() as usize);
            let now = &expansion["now"];
            let clock = Clock {
                date: now["date"].as_str().unwrap().into(),
                hour: now["hour"].as_u64().unwrap() as u8,
                minute: now["minute"].as_u64().unwrap() as u8,
            };
            let dismiss = expansion["dismiss"].as_bool().unwrap();
            let mut answers = HashMap::new();
            let mut asked = Vec::new();
            let result = loop {
                let context = Context {
                    clock: &clock,
                    page_title: expansion["title"].as_str().unwrap(),
                    days: &days,
                    answers: &answers,
                };
                match expand(copied, copy_parsed, &context) {
                    Ok(Step::Done(expanded)) => break Ok(expanded),
                    Ok(Step::Ask(question)) => {
                        let (kind, answer) = match &question {
                            quire_core::templates::Question::Ask(text) => {
                                ("ask", format!("A({text})"))
                            }
                            quire_core::templates::Question::Pick(tag) => {
                                ("pick", format!("P({tag})"))
                            }
                        };
                        asked.push(json!({"kind": kind, "question": question.text()}));
                        if dismiss {
                            break Err(CANCELLED.to_owned());
                        }
                        answers.insert(question.key(), answer);
                    }
                    Err(error) => break Err(error),
                }
            };
            let context = format!("{name} {key} into {destination} at {now}");
            assert_eq!(json!(asked), expansion["asked"], "{context} questions");
            match result {
                Ok(expanded) => {
                    if offered.contains(&text) {
                        assert!(
                            !LEFT_OVER.is_match(&expanded.text),
                            "{context} left a placeholder"
                        );
                        offered_checked += 1;
                    }
                    let offsets = Offsets::new(&expanded.text);
                    assert_eq!(
                        json!({"text": expanded.text, "cursor": offsets.unit(expanded.cursor)}),
                        expansion["result"],
                        "{context}"
                    );
                    let preserve = &expansion["preserve"];
                    let expanded_parsed = parsed(&expanded.text, &preserve["parsed"]);
                    assert_eq!(
                        preserve_root(&expanded.text, "trigger", &expanded_parsed),
                        preserve["text"],
                        "{context} preserved root"
                    );
                }
                Err(error) => assert_eq!(json!({"error": error}), expansion["result"], "{context}"),
            }
            checked += 1;
        }
    }
    assert!(checked > 800, "only {checked} expansions were checked");
    assert!(
        offered_checked >= 18,
        "only {offered_checked} starter and example expansions were checked"
    );
}

#[test]
fn date_phrases_match_the_reference() {
    for case in VECTORS["dates"].as_array().unwrap() {
        let result = template_date(
            case["phrase"].as_str().unwrap(),
            case["today"].as_str().unwrap(),
        );
        let actual = match result {
            Ok(date) => json!({"ok": date}),
            Err(error) => json!({"error": error}),
        };
        assert_eq!(actual, case["result"], "{case}");
    }
}

#[test]
fn past_names_rank_newest_then_most_used() {
    for case in VECTORS["past"].as_array().unwrap() {
        let uses: Vec<TagUse> = case["uses"]
            .as_array()
            .unwrap()
            .iter()
            .map(|usage| TagUse {
                path: usage["path"].as_str().unwrap().into(),
                title: usage["title"].as_str().unwrap().into(),
                links: usage["links"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|link| link.as_str().unwrap().into())
                    .collect(),
            })
            .collect();
        let names = past_names(&uses, &days(case["layout"].as_u64().unwrap() as usize));
        assert_eq!(
            json!(
                names
                    .iter()
                    .map(|name| json!({"name": name.name, "uses": name.uses, "last": name.last}))
                    .collect::<Vec<_>>()
            ),
            case["names"],
            "{}",
            case["tag"]
        );
    }
}

#[test]
fn fuzzy_matches_and_ranks_as_the_reference_does() {
    let fuzzy = &VECTORS["fuzzy"];
    for case in fuzzy["matches"].as_array().unwrap() {
        let actual = fuzzy_match(
            case["query"].as_str().unwrap(),
            case["name"].as_str().unwrap(),
        );
        let expected = &case["match"];
        let context = format!("{} in {}", case["query"], case["name"]);
        match actual {
            None => assert!(expected.is_null(), "{context}"),
            Some(found) => {
                assert_eq!(
                    found.score,
                    expected["score"].as_f64().unwrap(),
                    "{context}"
                );
                assert_eq!(json!(found.positions), expected["positions"], "{context}");
            }
        }
    }
    let names: Vec<&str> = fuzzy["names"]
        .as_array()
        .unwrap()
        .iter()
        .map(|name| name.as_str().unwrap())
        .collect();
    for case in fuzzy["filters"].as_array().unwrap() {
        let order: Vec<usize> = fuzzy_filter(&names, case["query"].as_str().unwrap(), |name| name)
            .into_iter()
            .map(|(order, _)| order)
            .collect();
        assert_eq!(json!(order), case["order"], "{}", case["query"]);
    }
}

#[test]
fn the_guide_matches_the_reference_list() {
    let expected = &VECTORS["guide"];
    let placeholders: Vec<Value> = guide::PLACEHOLDERS
        .iter()
        .map(|item| json!({"token": item.token, "meaning": item.meaning}))
        .collect();
    assert_eq!(json!(placeholders), expected["placeholders"]);
    assert_eq!(json!(guide::RULES), expected["rules"]);
    let examples: Vec<Value> = guide::EXAMPLES
        .iter()
        .map(|example| json!({"name": example.name, "use": example.usage, "body": example.body, "definition": guide::definition(example)}))
        .collect();
    assert_eq!(json!(examples), expected["examples"]);
    assert_eq!(guide::starter_templates(), expected["starters"]);
}

#[test]
fn a_changed_template_is_refused_and_the_catalogue_sorts_by_name() {
    let listed = quire_core::templates::Definition {
        path: "Templates.md".into(),
        key: "at-0".into(),
        name: "Meeting".into(),
        body: "- Agenda".into(),
    };
    let changed = quire_core::templates::Definition {
        body: "- Agenda\n- More".into(),
        ..listed.clone()
    };
    assert_eq!(current(&listed, std::slice::from_ref(&listed)), Ok(&listed));
    assert!(
        current(&listed, &[changed])
            .unwrap_err()
            .contains("changed")
    );
    assert!(current(&listed, &[]).is_err());
    let named = |name: &str| quire_core::templates::Definition {
        name: name.into(),
        ..listed.clone()
    };
    let sorted = catalogue(vec![
        named("my day"),
        named("Meeting"),
        named("Äpfel"),
        named("apple"),
    ]);
    assert_eq!(
        sorted
            .iter()
            .map(|item| item.name.as_str())
            .collect::<Vec<_>>(),
        ["Äpfel", "apple", "Meeting", "my day"]
    );
}

#[test]
fn placement_keeps_the_caret_and_indents_the_trigger_line() {
    let expanded = Expanded {
        text: "- First\n  - \n- Last\n".into(),
        cursor: 12,
    };
    assert_eq!(
        placement(&expanded, None, "    "),
        ("    - First\n      - \n    - Last".into(), 20)
    );
    let preserved = "- First ^trigger\n  - \n- Last\n";
    assert_eq!(
        placement(&expanded, Some(preserved), ""),
        ("- First ^trigger\n  - \n- Last".into(), 21)
    );
    let at_end = Expanded {
        text: "- Plain\n".into(),
        cursor: 7,
    };
    assert_eq!(
        placement(&at_end, Some("- Plain ^trigger\n"), ""),
        ("- Plain ^trigger".into(), 7)
    );
    let replaced = Expanded {
        text: "- One ^b-1\n- ((b-1))\n".into(),
        cursor: 21,
    };
    assert_eq!(
        placement(&replaced, Some("- One ^trigger\n- ((trigger))\n"), "  "),
        ("  - One ^trigger\n  - ((trigger))".into(), 32)
    );
}

/// Regression: the reference trims the space after a final `{cursor}` bullet, leaving `  -`
/// with the caret after the dash, so typed text no longer belonged to the bullet.
#[test]
fn a_cursor_on_the_last_bullet_keeps_its_space() {
    let expanded = Expanded {
        text: "- Meeting\n  - \n".into(),
        cursor: 14,
    };
    assert_eq!(
        placement(&expanded, None, "\t"),
        ("\t- Meeting\n\t  - ".into(), 16)
    );
    let trailing = Expanded {
        text: "- Note  \n".into(),
        cursor: 6,
    };
    assert_eq!(placement(&trailing, None, ""), ("- Note".into(), 6));
}

#[test]
fn the_template_clock_reads_local_wall_time() {
    let now = 1_793_361_600_000;
    assert_eq!(
        DailyCalendar::utc().clock_at(now),
        Ok(("2026-10-30".into(), 12, 0))
    );
    assert_eq!(
        DailyCalendar::named("America/New_York")
            .unwrap()
            .clock_at(now),
        Ok(("2026-10-30".into(), 8, 0))
    );
    assert_eq!(
        DailyCalendar::named("Pacific/Kiritimati")
            .unwrap()
            .clock_at(now),
        Ok(("2026-10-31".into(), 2, 0))
    );
}
