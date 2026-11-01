//! Differential tests against vectors from the frozen rt task modules
//! (`tools/task-vectors.ts`). The reference analysis supplies blocks, tags, references, and
//! syntax ranges, so these tests check task semantics alone; the native analyzer is checked
//! end to end in the quire crate.

use quire_core::daily::{DailyCalendar, DailyLayout};
use quire_core::markdown::{Reference, ReferenceKind, ReferenceMedia, ReferenceSyntax, TagToken};
use quire_core::tasks::{
    Action, DailyNames, DateField, Edit, Page, PageBlock, Shortcut, Task, TaskFilter, TaskState,
    add_days, capture, created_stamp, is_due, is_upcoming, next_week, project_membership,
    short_date, task_at, task_date_label, tasks_in,
};
use serde_json::{Value, json};
use std::{ops::Range, sync::LazyLock};

static VECTORS: LazyLock<Value> = LazyLock::new(|| {
    serde_json::from_str(include_str!("../../../tests/fixtures/task-vectors.json")).unwrap()
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

    fn range(&self, value: &Value) -> Range<usize> {
        if let Some(pair) = value.as_array() {
            self.byte(&pair[0])..self.byte(&pair[1])
        } else {
            self.byte(&value["from"])..self.byte(&value["to"])
        }
    }

    fn unit(&self, byte: usize) -> usize {
        self.units[byte]
    }

    fn edits(&self, edits: &[Edit]) -> Value {
        json!(
            edits
                .iter()
                .map(|edit| json!({"from": self.unit(edit.range.start), "to": self.unit(edit.range.end), "insert": edit.replacement}))
                .collect::<Vec<_>>()
        )
    }
}

fn days(layout: &Value) -> DailyNames {
    DailyNames::new(
        DailyLayout {
            folder: layout["folder"].as_str().unwrap().into(),
            title: layout["title"].as_str().unwrap().into(),
        },
        DailyCalendar::utc(),
    )
}

fn task_json(task: &Task, offsets: &Offsets) -> Value {
    json!({
        "key": format!("page.md:{}:{}", task.block_key, offsets.unit(task.block_from)),
        "path": "page.md",
        "blockKey": task.block_key,
        "id": task.id,
        "from": offsets.unit(task.content.start),
        "to": offsets.unit(task.content.end),
        "raw": task.raw,
        "text": task.text,
        "marker": task.marker,
        "checked": task.checked,
        "state": task.state.as_str(),
        "tags": task.tags,
        "links": task.links,
        "created": task.created,
        "due": task.due,
        "completed": task.completed,
    })
}

fn outcome<T, E: ToString>(result: Result<T, E>, ok: impl FnOnce(T) -> Value) -> Value {
    match result {
        Ok(value) => json!({"ok": ok(value)}),
        Err(error) => json!({"error": error.to_string()}),
    }
}

fn shortcut(name: &Value) -> Shortcut {
    match name.as_str().unwrap() {
        "toggle" => Shortcut::Toggle,
        _ => Shortcut::Cycle,
    }
}

struct OwnedBlock {
    key: String,
    id: Option<String>,
    kind: String,
    from: usize,
    content: (usize, usize),
    parent: Option<String>,
}

struct Inputs {
    blocks: Vec<OwnedBlock>,
    tags: Vec<TagToken>,
    references: Vec<Reference>,
    syntax: Vec<Range<usize>>,
    code: Vec<Range<usize>>,
}

fn inputs(case: &Value, offsets: &Offsets) -> Inputs {
    let inputs = &case["inputs"];
    let text = case["text"].as_str().unwrap();
    let string = |value: &Value| value.as_str().map(str::to_owned);
    Inputs {
        blocks: inputs["blocks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|block| {
                let content =
                    offsets.byte(&block["contentFrom"])..offsets.byte(&block["contentTo"]);
                assert_eq!(offsets.byte(&block["to"]), content.end);
                let parsed = PageBlock {
                    key: "",
                    id: None,
                    kind: "",
                    from: 0,
                    content: (content.start, content.end),
                    parent: None,
                };
                assert_eq!(parsed.text(text), block["text"].as_str().unwrap());
                OwnedBlock {
                    key: string(&block["key"]).unwrap(),
                    id: string(&block["id"]),
                    kind: string(&block["kind"]).unwrap(),
                    from: offsets.byte(&block["from"]),
                    content: (content.start, content.end),
                    parent: string(&block["parent"]),
                }
            })
            .collect(),
        tags: inputs["tags"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tag| TagToken {
                range: offsets.range(tag),
                text: string(&tag["text"]).unwrap(),
            })
            .collect(),
        references: inputs["references"]
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
                range: offsets.range(reference),
                target: string(&reference["target"]).unwrap(),
                label: string(&reference["label"]),
                media: reference["media"].as_str().map(|_| ReferenceMedia::Pdf),
            })
            .collect(),
        syntax: inputs["syntaxRanges"]
            .as_array()
            .unwrap()
            .iter()
            .map(|range| offsets.range(range))
            .collect(),
        code: inputs["codeRanges"]
            .as_array()
            .unwrap()
            .iter()
            .map(|range| offsets.range(range))
            .collect(),
    }
}

#[test]
fn pages_match_the_frozen_task_vectors() {
    let today = VECTORS["today"].as_str().unwrap();
    let mut failures = Vec::new();
    for case in VECTORS["cases"].as_array().unwrap() {
        let text = case["text"].as_str().unwrap();
        let offsets = Offsets::new(text);
        let inputs = inputs(case, &offsets);
        let blocks: Vec<PageBlock> = inputs
            .blocks
            .iter()
            .map(|block| PageBlock {
                key: &block.key,
                id: block.id.as_deref(),
                kind: &block.kind,
                from: block.from,
                content: block.content,
                parent: block.parent.as_deref(),
            })
            .collect();
        let page = Page {
            text,
            title: case["title"].as_str().unwrap(),
            blocks: &blocks,
            tags: &inputs.tags,
            references: &inputs.references,
            syntax_ranges: &inputs.syntax,
            code_ranges: &inputs.code,
        };
        let days = days(&case["layout"]);
        let tasks = page.tasks(&days);
        let mut check = |what: String, expected: &Value, actual: Value| {
            if *expected != actual {
                failures.push(format!(
                    "{} {what} text={text:?}\nexpected={expected}\nactual={actual}",
                    case["name"]
                ));
            }
        };
        let mut expected_tasks = case["tasks"].clone();
        let duplicate_keys = {
            let mut keys: Vec<_> = blocks.iter().map(|block| block.key).collect();
            keys.sort_unstable();
            keys.windows(2).any(|pair| pair[0] == pair[1])
        };
        if duplicate_keys {
            // The reference caches a block's links by key, so a second block with the same ID
            // reads the first one's links. Quire reads each block's own links.
            for (expected, task) in expected_tasks
                .as_array_mut()
                .unwrap()
                .iter_mut()
                .zip(&tasks)
            {
                expected["links"] = json!(task.links);
            }
        }
        check(
            "tasks".into(),
            &expected_tasks,
            json!(
                tasks
                    .iter()
                    .map(|task| task_json(task, &offsets))
                    .collect::<Vec<_>>()
            ),
        );
        check(
            "stamp".into(),
            &case["stamp"],
            json!(created_stamp(page.title, today, &days)),
        );
        for (task, expected) in tasks.iter().zip(case["changes"].as_array().unwrap()) {
            for (name, action) in [
                ("toggle", Action::Toggle),
                ("inbox", Action::Set(TaskState::Inbox)),
                ("next", Action::Set(TaskState::Next)),
                ("waiting", Action::Set(TaskState::Waiting)),
                ("someday", Action::Set(TaskState::Someday)),
                ("done", Action::Set(TaskState::Done)),
            ] {
                check(
                    format!("changes {} {name}", task.raw),
                    &expected[name],
                    offsets.edits(&task.changes(action, today)),
                );
            }
        }
        for (task, expected) in tasks.iter().zip(case["dates"].as_array().unwrap()) {
            for vector in expected.as_array().unwrap() {
                let field = match vector["field"].as_str().unwrap() {
                    "created" => DateField::Created,
                    "due" => DateField::Due,
                    _ => DateField::Completed,
                };
                let result = task.set_date(field, vector["date"].as_str(), &days);
                check(
                    format!("date {} {vector}", task.raw),
                    &vector["result"],
                    outcome(result, |edits| offsets.edits(&edits)),
                );
            }
        }
        for vector in case["taskAt"].as_array().unwrap() {
            let position = offsets.byte(&vector["position"]);
            let result = task_at(
                &page,
                &tasks,
                &days,
                today,
                position,
                (position, position),
                shortcut(&vector["action"]),
            );
            let actual = outcome(result, |edit| {
                let mut value = json!({"changes": offsets.edits(&edit.edits)});
                if let Some((anchor, head)) = edit.selection {
                    let after = Offsets::new(&applied(text, &edit.edits));
                    value["selection"] =
                        json!({"anchor": after.unit(anchor), "head": after.unit(head)});
                }
                value
            });
            check(format!("taskAt {vector}"), &vector["result"], actual);
        }
        for vector in case["tasksIn"].as_array().unwrap() {
            let range = offsets.byte(&vector["from"])..offsets.byte(&vector["to"]);
            let result = tasks_in(
                &page,
                &tasks,
                &days,
                today,
                range,
                shortcut(&vector["action"]),
            );
            let actual = outcome(result, |edits| offsets.edits(&edits));
            check(format!("tasksIn {vector}"), &vector["result"], actual);
        }
    }
    assert!(
        failures.is_empty(),
        "{} task vector mismatches:\n{}",
        failures.len(),
        failures
            .iter()
            .take(10)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n\n")
    );
}

#[test]
fn captures_match_the_frozen_vectors() {
    for vector in VECTORS["captures"].as_array().unwrap() {
        let current = vector["current"].as_str().unwrap();
        let value = vector["value"].as_str().unwrap();
        let offsets = Offsets::new(value);
        let current_offsets = Offsets::new(current);
        let tags: Vec<TagToken> = vector["valueTags"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tag| TagToken {
                range: offsets.range(tag),
                text: tag["text"].as_str().unwrap().into(),
            })
            .collect();
        let result = capture(
            current,
            value,
            &tags,
            vector["link"].as_str(),
            vector["stamp"].as_str().unwrap(),
        );
        assert_eq!(
            outcome(result, |edit| current_offsets.edits(&[edit])[0].clone()),
            vector["result"],
            "{vector}"
        );
    }
}

fn dated(state: TaskState, due: Option<&str>, completed: Option<&str>) -> Task {
    Task {
        block_key: String::new(),
        block_from: 0,
        id: None,
        content: 0..0,
        raw: String::new(),
        text: String::new(),
        marker: "[ ]".into(),
        checked: false,
        state,
        tags: Vec::new(),
        links: Vec::new(),
        created: None,
        due: due.map(str::to_owned),
        completed: completed.map(str::to_owned),
        tag_tokens: Vec::new(),
    }
}

#[test]
fn date_helpers_match_the_frozen_vectors() {
    let dates = &VECTORS["dates"];
    for vector in dates["addDays"].as_array().unwrap() {
        assert_eq!(
            add_days(
                vector["iso"].as_str().unwrap(),
                vector["days"].as_i64().unwrap()
            )
            .as_deref(),
            vector["result"].as_str(),
            "{vector}"
        );
    }
    for vector in dates["upcoming"].as_array().unwrap() {
        let state = TaskState::parse(vector["state"].as_str().unwrap()).unwrap();
        let task = dated(state, vector["due"].as_str(), None);
        let today = vector["today"].as_str().unwrap();
        assert_eq!(is_due(&task, today), vector["due_now"], "{vector}");
        assert_eq!(is_upcoming(&task, today, 7), vector["upcoming"], "{vector}");
    }
    for vector in dates["short"].as_array().unwrap() {
        assert_eq!(
            short_date(
                vector["iso"].as_str().unwrap(),
                vector["today"].as_str().unwrap()
            ),
            vector["result"].as_str().unwrap()
        );
    }
    for vector in dates["labels"].as_array().unwrap() {
        let state = TaskState::parse(vector["state"].as_str().unwrap()).unwrap();
        let task = dated(state, vector["due"].as_str(), vector["completed"].as_str());
        assert_eq!(
            task_date_label(&task, VECTORS["today"].as_str().unwrap()).as_deref(),
            vector["result"].as_str(),
            "{vector}"
        );
    }
}

#[test]
fn filters_match_the_frozen_vectors() {
    let tasks = VECTORS["filterTasks"].as_array().unwrap();
    for vector in VECTORS["filters"].as_array().unwrap() {
        let filter = TaskFilter::new(vector["query"].as_str().unwrap());
        let actual: Vec<bool> = tasks
            .iter()
            .map(|task| {
                let tags: Vec<String> = task[1]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|tag| tag.as_str().unwrap().into())
                    .collect();
                filter.matches_parts(task[0].as_str().unwrap(), &tags, "Daily")
            })
            .collect();
        assert_eq!(json!(actual), vector["results"], "{vector}");
    }
}

#[test]
fn projects_come_from_the_page_its_links_and_front_matter_project_tags() {
    let projects = vec![
        ("Projects/Atlas.md".to_owned(), vec![]),
        ("Launch.md".to_owned(), vec!["project/launch".to_owned()]),
        ("Other.md".to_owned(), vec!["Project/Launch".to_owned()]),
    ];
    let mut task = dated(TaskState::Next, None, None);
    task.links = vec!["Atlas".into(), "Missing".into(), "Projects/Atlas".into()];
    task.tags = vec!["#project/launch".into(), "#next".into()];
    let find = |link: &str| (link == "Projects/Atlas").then(|| "Projects/Atlas.md".to_owned());
    assert_eq!(
        project_membership("Launch.md", &task, &projects, find),
        ["Launch.md", "Projects/Atlas.md", "Other.md"]
    );
    task.tags.clear();
    task.links.clear();
    assert!(project_membership("Inbox.md", &task, &projects, find).is_empty());
}

#[test]
fn task_markers_preserve_legacy_spelling_and_boundaries() {
    use quire_core::tasks::read_marker;
    for (input, checked, toggled, prefix) in [
        ("[ ] task", false, "[x]", 4),
        ("[X]\tمرحبا", true, "[ ]", 4),
        ("{{[[TODO]]}} task", false, "{{[[DONE]]}}", 13),
        ("{{DONE}}", true, "{{TODO}}", 8),
        ("{{[[TODO}} task", false, "{{[[DONE}}", 11),
        ("{{DONE]]}}  \t task", true, "{{TODO]]}}", 14),
    ] {
        let marker = read_marker(input).unwrap();
        assert_eq!(marker.checked, checked);
        assert_eq!(marker.toggled(), toggled);
        assert_eq!(marker.prefix_len, prefix, "{input:?}");
    }
    for input in [
        "[x]body",
        "[-] task",
        "[ ]\nnext",
        " {{TODO}} task",
        "{{todo}} task",
        "{{TODO}}🙂",
        "[",
        "[ ",
        "{{",
        "[🙂] task",
    ] {
        assert!(read_marker(input).is_none(), "{input:?}");
    }
}

#[test]
fn a_repeated_block_id_reads_its_own_links_not_the_first_blocks() {
    let text = "- Notes [[Alpha]] ^same\n  - [ ] Child [[Beta]] ^same\n";
    let blocks = [
        PageBlock {
            key: "same",
            id: Some("same"),
            kind: "list",
            from: 0,
            content: (2, 23),
            parent: None,
        },
        PageBlock {
            key: "same",
            id: Some("same"),
            kind: "list",
            from: 26,
            content: (28, 51),
            parent: Some("same"),
        },
    ];
    let link = |target: &str| {
        let start = text.find(&format!("[[{target}]]")).unwrap();
        Reference {
            kind: ReferenceKind::Page,
            syntax: ReferenceSyntax::Wiki,
            range: start..start + target.len() + 4,
            target: target.into(),
            label: None,
            media: None,
        }
    };
    let references = [link("Alpha"), link("Beta")];
    let page = Page {
        text,
        title: "Page",
        blocks: &blocks,
        tags: &[],
        references: &references,
        syntax_ranges: &[],
        code_ranges: &[],
    };
    let days = DailyNames::new(DailyLayout::default(), DailyCalendar::utc());
    assert_eq!(page.tasks(&days)[0].links, ["Beta"]);
}

fn applied(text: &str, edits: &[Edit]) -> String {
    let mut result = text.to_owned();
    for edit in edits.iter().rev() {
        result.replace_range(edit.range.clone(), &edit.replacement);
    }
    result
}

#[test]
fn undated_tasks_keep_date_sources_that_any_layout_resolves() {
    let text =
        "- Plan [[Oct 30th, 2026]]\n  - [ ] Child [[02.11.2026]] 📅 [[nope]] ➕ [[2026-10-01]]\n";
    let child_from = text.find("  - [ ]").unwrap() + 2;
    let blocks = [
        PageBlock {
            key: "a",
            id: None,
            kind: "list",
            from: 0,
            content: (2, text.find('\n').unwrap()),
            parent: None,
        },
        PageBlock {
            key: "b",
            id: None,
            kind: "list",
            from: child_from,
            content: (child_from + 2, text.len() - 1),
            parent: Some("a"),
        },
    ];
    let link = |target: &str| {
        let start = text.find(&format!("[[{target}]]")).unwrap();
        Reference {
            kind: ReferenceKind::Page,
            syntax: ReferenceSyntax::Wiki,
            range: start..start + target.len() + 4,
            target: target.into(),
            label: None,
            media: None,
        }
    };
    let references = [
        link("Oct 30th, 2026"),
        link("02.11.2026"),
        link("nope"),
        link("2026-10-01"),
    ];
    let page = Page {
        text,
        title: "Plan",
        blocks: &blocks,
        tags: &[],
        references: &references,
        syntax_ranges: &[],
        code_ranges: &[],
    };
    let undated = page.undated_tasks();
    let (task, sources) = &undated[0];
    assert_eq!((task.created.as_deref(), task.due.as_deref()), (None, None));
    assert_eq!(sources.due.as_deref(), Some("nope"));
    assert_eq!(sources.created.as_deref(), Some("2026-10-01"));
    assert_eq!(sources.links, ["02.11.2026", "nope"]);
    assert_eq!(sources.inherited, ["Oct 30th, 2026"]);
    let dotted = days(&json!({"folder": "", "title": "{DD}.{MM}.{YYYY}"}));
    let iso = days(&json!({"folder": "", "title": "{YYYY}-{MM}-{DD}"}));
    for layout in [&dotted, &iso] {
        let resolved = sources.resolve(task.clone(), layout, None);
        assert_eq!(resolved, page.tasks(layout)[0]);
    }
    assert_eq!(page.tasks(&dotted)[0].due.as_deref(), Some("2026-11-02"));
    assert_eq!(
        page.tasks(&iso)[0].due.as_deref(),
        Some("2026-10-30"),
        "without a dated link of its own the task inherits its parent's Roam date"
    );
    assert_eq!(
        sources
            .resolve(task.clone(), &iso, Some("2026-12-24"))
            .created
            .as_deref(),
        Some("2026-10-01")
    );
}

#[test]
fn typed_dates_read_relative_phrases_weekdays_and_iso_dates() {
    let today = "2026-10-29";
    for (phrase, expected) in [
        ("", None),
        ("  ", None),
        ("today", Some("2026-10-29")),
        ("Tomorrow", Some("2026-10-30")),
        ("yesterday", Some("2026-10-28")),
        ("+3", Some("2026-11-01")),
        ("-29", Some("2026-09-30")),
        ("fri", Some("2026-10-30")),
        ("thursday", Some("2026-11-05")),
        ("Mon", Some("2026-11-02")),
        ("2027-02-28", Some("2027-02-28")),
    ] {
        assert_eq!(
            quire_core::tasks::date_phrase(phrase, today)
                .unwrap()
                .as_deref(),
            expected,
            "{phrase:?}"
        );
    }
    for phrase in ["soon", "fr", "+x", "2026/10/01"] {
        let error = quire_core::tasks::date_phrase(phrase, today).unwrap_err();
        assert!(error.contains("is not a date"), "{phrase:?}: {error}");
    }
}

#[test]
fn next_week_is_the_monday_after_today() {
    // 2026-11-01 is a Sunday.
    assert_eq!(next_week("2026-11-01").as_deref(), Some("2026-11-02"));
    assert_eq!(next_week("2026-11-02").as_deref(), Some("2026-11-09"));
    assert_eq!(next_week("2026-11-06").as_deref(), Some("2026-11-09"));
    assert_eq!(next_week("2026-12-29").as_deref(), Some("2027-01-04"));
    assert_eq!(next_week("not a date"), None);
}
