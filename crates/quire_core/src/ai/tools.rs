//! The notebook tools AI command line tools call, ported from the reference
//! `notebook-tool-specs.ts`: names, descriptions, and input schemas (a contract with terminal
//! users, kept identical to the reference), the input check, and the shared result helpers the
//! app's tool host uses.

use serde_json::{Map, Value, json};

/// Whether write tools are offered, and whether their changes wait for review.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Writes {
    Off,
    #[default]
    Review,
    Apply,
}

impl Writes {
    pub fn key(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Review => "review",
            Self::Apply => "apply",
        }
    }
}

pub const WRITES_OFF: &str = "AI writes are off for this notebook. Turn on AI writes in Quire Settings > AI, then ask again.";
pub const AI_COMMANDS_OFF: &str =
    "AI commands are off in Quire. Turn them on in Settings > AI, then retry.";
pub const NO_NOTEBOOK: &str =
    "No notebook is open in Quire. Open the notebook in Quire, then retry.";
pub const AWAITING: &str = "The writer reviews this change in Quire before it applies. Carry on as if it will apply and do not repeat the call.";

/// Most items a list tool returns.
pub const CAP: usize = 200;
/// Most characters of page text one call returns.
pub const TEXT_CAP: usize = 60_000;

pub const READ_TOOLS: [&str; 9] = [
    "search_notebook",
    "list_pages",
    "read_page",
    "read_block",
    "linked_references",
    "tagged_blocks",
    "tasks",
    "daily_pages",
    "mentions",
];
pub const WRITE_TOOLS: [&str; 5] = [
    "append_block",
    "create_page",
    "replace_block",
    "set_task_state",
    "capture_task",
];
const TASK_STATES: [&str; 5] = ["inbox", "next", "waiting", "someday", "done"];

pub fn is_write(name: &str) -> bool {
    WRITE_TOOLS.contains(&name)
}

fn date(description: &str) -> Value {
    json!({
        "type": "string",
        "pattern": "^(\\d{4}-\\d{2}-\\d{2}|today)$",
        "description": format!("{description} As YYYY-MM-DD or \"today\"."),
    })
}

fn page() -> Value {
    json!({
        "type": "string",
        "minLength": 1,
        "maxLength": 500,
        "description": "Page title, notebook-relative path such as \"Projects/Plan.md\", or \"today\" for today's daily page.",
    })
}

fn limit() -> Value {
    json!({
        "type": "integer",
        "minimum": 1,
        "maximum": 200,
        "description": "Most items to return. Default 50, at most 200.",
    })
}

fn text(min: Option<u32>, max: u32) -> Value {
    let mut schema = Map::new();
    schema.insert("type".into(), "string".into());
    if let Some(min) = min {
        schema.insert("minLength".into(), min.into());
    }
    schema.insert("maxLength".into(), max.into());
    Value::Object(schema)
}

fn object(properties: Vec<(&str, Value)>, required: &[&str]) -> Value {
    let properties: Map<String, Value> = properties
        .into_iter()
        .map(|(key, value)| (key.to_owned(), value))
        .collect();
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false,
    })
}

fn states(extra: &[&str]) -> Value {
    let all: Vec<&str> = TASK_STATES.iter().chain(extra).copied().collect();
    json!({ "type": "string", "enum": all })
}

/// One tool: its name, description, input schema, and whether it writes.
#[derive(Clone, Debug)]
pub struct ToolSpec {
    pub name: &'static str,
    pub description: &'static str,
    pub write: bool,
    pub input_schema: Value,
}

/// Every tool, read tools first, in the reference order.
pub fn tool_specs() -> Vec<ToolSpec> {
    let spec = |name, description, write, input_schema| ToolSpec {
        name,
        description,
        write,
        input_schema,
    };
    vec![
        spec(
            "search_notebook",
            "Full-text search over every page and block. Returns matching blocks with page title, path, block id and a snippet. Use it to find pages or blocks by words they contain; use tasks, tagged_blocks or mentions for structured questions.",
            false,
            object(
                vec![
                    (
                        "query",
                        json!({ "type": "string", "minLength": 2, "maxLength": 300, "description": "Words to find." }),
                    ),
                    ("limit", limit()),
                ],
                &["query"],
            ),
        ),
        spec(
            "list_pages",
            "Lists pages with title, path, daily date (for daily pages), last change time, task count and incoming reference count. Filter by namespace (the part of a title before the last /, or the page's folder such as People), title prefix, words anywhere in the title (contains), daily date range or change date. Sort by title, date (daily date, newest first) or modified (newest first).",
            false,
            object(
                vec![
                    ("namespace", text(None, 300)),
                    ("prefix", text(None, 300)),
                    ("contains", text(None, 300)),
                    ("daily_from", date("Earliest daily date.")),
                    ("daily_to", date("Latest daily date.")),
                    (
                        "modified_after",
                        date("Only pages changed on or after this date."),
                    ),
                    (
                        "sort",
                        json!({ "type": "string", "enum": ["title", "date", "modified"] }),
                    ),
                    ("limit", limit()),
                ],
                &[],
            ),
        ),
        spec(
            "read_page",
            "Reads one page as Markdown with its block ids (^id at the end of a line) and a key for every block (by line), plus its tags, task count, incoming reference count and daily date. Long pages are cut at 60,000 characters; pass from_line and to_line to read the rest.",
            false,
            object(
                vec![
                    ("page", page()),
                    ("from_line", json!({ "type": "integer", "minimum": 1 })),
                    ("to_line", json!({ "type": "integer", "minimum": 1 })),
                ],
                &["page"],
            ),
        ),
        spec(
            "read_block",
            "Reads one block by its id with its child blocks, the page it is on and the text of its parent blocks.",
            false,
            object(vec![("id", text(Some(1), 100))], &["id"]),
        ),
        spec(
            "linked_references",
            "Lists the blocks on other pages that link to a page or to one of its blocks, newest daily page first, with each block's text.",
            false,
            object(vec![("page", page()), ("limit", limit())], &["page"]),
        ),
        spec(
            "tagged_blocks",
            "Lists blocks carrying a #tag with their page, date (the page's daily date, or the day the page last changed) and task state. Optional date range and status filter.",
            false,
            object(
                vec![
                    (
                        "tag",
                        json!({ "type": "string", "minLength": 1, "maxLength": 200, "description": "Tag with or without #." }),
                    ),
                    ("from", date("Earliest date.")),
                    ("to", date("Latest date.")),
                    (
                        "status",
                        json!({ "type": "string", "enum": ["all", "open", "done", "due"] }),
                    ),
                    ("limit", limit()),
                ],
                &["tag"],
            ),
        ),
        spec(
            "tasks",
            "Lists tasks (checkbox list items) with state, due date, completion date, tags, page title and key. state \"open\" means not done. due: \"today\" (due today), \"overdue\" (before today and open), \"week\" (today through 6 days ahead), or \"range\" with from and to. A task is due on the daily page it links to or sits on.",
            false,
            object(
                vec![
                    ("state", states(&["open"])),
                    (
                        "due",
                        json!({ "type": "string", "enum": ["today", "overdue", "week", "range"] }),
                    ),
                    ("from", date("Range start.")),
                    ("to", date("Range end.")),
                    (
                        "project",
                        json!({ "type": "string", "maxLength": 300, "description": "Project page title." }),
                    ),
                    ("tag", text(None, 200)),
                    ("page", page()),
                    ("text", text(None, 300)),
                    ("limit", limit()),
                ],
                &[],
            ),
        ),
        spec(
            "daily_pages",
            "Returns the text of daily pages between two dates, oldest first, up to 60,000 characters in total.",
            false,
            object(
                vec![("from", date("First day.")), ("to", date("Last day."))],
                &["from", "to"],
            ),
        ),
        spec(
            "mentions",
            "Lists the pages that link to a page, newest daily page first, with how often each links it. Use it for \"the last time we discussed X\" or \"the last occurrence of a meeting\".",
            false,
            object(vec![("page", page()), ("limit", limit())], &["page"]),
        ),
        spec(
            "append_block",
            "Adds Markdown as a new block at the end of a page, or of today's daily page. The change may wait for the writer's review; the result says whether it applied.",
            true,
            object(
                vec![("page", page()), ("text", text(Some(1), 20000))],
                &["page", "text"],
            ),
        ),
        spec(
            "create_page",
            "Creates a new page with a title and Markdown text.",
            true,
            object(
                vec![("title", text(Some(1), 200)), ("text", text(None, 20000))],
                &["title"],
            ),
        ),
        spec(
            "replace_block",
            "Replaces the text of one block, keeping its id and child blocks. Find it by id, or, for a block without one, by the key and page read_page listed. Pass the new text without the ^id marker. No id is added to a block found by key.",
            true,
            object(
                vec![
                    ("id", text(Some(1), 100)),
                    ("key", text(Some(1), 100)),
                    ("page", page()),
                    ("text", text(Some(1), 20000)),
                ],
                &["text"],
            ),
        ),
        spec(
            "set_task_state",
            "Changes a task's state. Pass the key from the tasks tool. Completing appends today's completion date.",
            true,
            object(
                vec![("key", text(Some(1), 1000)), ("state", states(&[]))],
                &["key", "state"],
            ),
        ),
        spec(
            "capture_task",
            "Adds one task to the Inbox page, or to today's daily page when the writer chose that in Settings, optionally linked to a project page.",
            true,
            object(
                vec![("text", text(Some(1), 1000)), ("project", text(None, 300))],
                &["text"],
            ),
        ),
    ]
}

pub fn spec(name: &str) -> Option<ToolSpec> {
    tool_specs().into_iter().find(|spec| spec.name == name)
}

/// The tools as MCP lists them, without write tools while AI writes are off.
pub fn listed_tools(writes: Writes) -> Value {
    Value::Array(
        tool_specs()
            .into_iter()
            .filter(|tool| writes != Writes::Off || !tool.write)
            .map(|tool| {
                json!({
                    "name": tool.name,
                    "description": tool.description,
                    "inputSchema": tool.input_schema,
                })
            })
            .collect(),
    )
}

fn utf16_len(text: &str) -> u64 {
    text.encode_utf16().count() as u64
}

fn whole(value: &Value) -> Option<f64> {
    let number = value.as_f64()?;
    (number.fract() == 0.0 && number.is_finite()).then_some(number)
}

fn number_text(value: &Value) -> String {
    value.to_string()
}

/// Checks `value` against a tool's schema, as the reference `validateInput` does; returns what is
/// wrong, or None.
pub fn validate_input(schema: &Value, value: &Value, at: &str) -> Option<String> {
    let kind = schema.get("type").and_then(Value::as_str).unwrap_or("");
    match kind {
        "object" => {
            let Some(record) = value.as_object() else {
                return Some(format!("{at} must be an object."));
            };
            let required = schema.get("required").and_then(Value::as_array);
            for key in required.into_iter().flatten().filter_map(Value::as_str) {
                if !record.contains_key(key) {
                    return Some(format!("{at}.{key} is required."));
                }
            }
            let properties = schema.get("properties").and_then(Value::as_object);
            for (key, item) in record {
                let Some(property) = properties.and_then(|properties| properties.get(key)) else {
                    if schema.get("additionalProperties") == Some(&Value::Bool(false)) {
                        return Some(format!("{at}.{key} is not a known field."));
                    }
                    continue;
                };
                if let Some(problem) = validate_input(property, item, &format!("{at}.{key}")) {
                    return Some(problem);
                }
            }
            None
        }
        "array" => {
            let Some(items) = value.as_array() else {
                return Some(format!("{at} must be a list."));
            };
            let item_schema = schema.get("items")?;
            items.iter().enumerate().find_map(|(index, item)| {
                validate_input(item_schema, item, &format!("{at}[{index}]"))
            })
        }
        "boolean" => (!value.is_boolean()).then(|| format!("{at} must be true or false.")),
        "integer" => {
            let Some(number) = whole(value) else {
                return Some(format!("{at} must be a whole number."));
            };
            if let Some(minimum) = schema.get("minimum").and_then(Value::as_f64)
                && number < minimum
            {
                return Some(format!(
                    "{at} must be at least {}.",
                    number_text(&schema["minimum"])
                ));
            }
            if let Some(maximum) = schema.get("maximum").and_then(Value::as_f64)
                && number > maximum
            {
                return Some(format!(
                    "{at} must be at most {}.",
                    number_text(&schema["maximum"])
                ));
            }
            None
        }
        _ => {
            let Some(text) = value.as_str() else {
                return Some(format!("{at} must be text."));
            };
            let length = utf16_len(text);
            if let Some(min) = schema.get("minLength").and_then(Value::as_u64)
                && length < min
            {
                return Some(format!("{at} must have at least {min} characters."));
            }
            if let Some(max) = schema.get("maxLength").and_then(Value::as_u64)
                && length > max
            {
                return Some(format!("{at} must have at most {max} characters."));
            }
            if let Some(choices) = schema.get("enum").and_then(Value::as_array)
                && !choices.iter().any(|choice| choice.as_str() == Some(text))
            {
                let names: Vec<&str> = choices.iter().filter_map(Value::as_str).collect();
                return Some(format!("{at} must be one of {}.", names.join(", ")));
            }
            if let Some(pattern) = schema.get("pattern").and_then(Value::as_str)
                && !regex::RegexBuilder::new(pattern)
                    .unicode(false)
                    .build()
                    .is_ok_and(|pattern| pattern.is_match(text))
            {
                let description = schema
                    .get("description")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                return Some(
                    format!("{at} has the wrong format. {description}")
                        .trim()
                        .to_owned(),
                );
            }
            None
        }
    }
}

/// Checks a call's input against its tool, as both the bridge and the tool host do.
pub fn check_input(name: &str, input: &Value) -> Result<ToolSpec, String> {
    let names: Vec<&str> = READ_TOOLS.iter().chain(&WRITE_TOOLS).copied().collect();
    let spec = spec(name).ok_or_else(|| {
        format!(
            "Unknown tool {name}. Available tools: {}.",
            names.join(", ")
        )
    })?;
    match validate_input(&spec.input_schema, input, "input") {
        Some(problem) => Err(format!("{problem} Check the {name} input schema.")),
        None => Ok(spec),
    }
}

/// A list cut to `limit` (50 by default, at most 200), with the note the reference adds when
/// items were left out.
pub fn take<T>(mut items: Vec<T>, limit: Option<u64>) -> (Vec<T>, Option<String>) {
    let count = limit.map_or(50, |limit| limit as usize).min(CAP);
    let total = items.len();
    items.truncate(count);
    let truncated = (total > count).then(|| {
        format!("Showing {count} of {total}. Narrow the request or raise limit (at most {CAP}).")
    });
    (items, truncated)
}

pub fn plural(count: usize, word: &str) -> String {
    format!("{count} {word}{}", if count == 1 { "" } else { "s" })
}

/// The reference block key: the block's 1-based line and an FNV-1a hash of its text in base 36.
/// It finds a block without an ID and stops matching once the block's text changes.
pub fn block_key(line: usize, text: &str) -> String {
    let hash = text.encode_utf16().fold(0x811c_9dc5_u32, |hash, unit| {
        (hash ^ u32::from(unit)).wrapping_mul(0x0100_0193)
    });
    format!("L{line}-{}", base36(hash))
}

fn base36(mut value: u32) -> String {
    const DIGITS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut out = Vec::new();
    loop {
        out.push(DIGITS[(value % 36) as usize]);
        value /= 36;
        if value == 0 {
            break;
        }
    }
    out.reverse();
    String::from_utf8(out).unwrap_or_default()
}

/// The 1-based line of a UTF-16 offset in `text`.
pub fn line_at(text: &str, utf16: usize) -> usize {
    let mut units = 0;
    let mut line = 1;
    for character in text.chars() {
        if units >= utf16 {
            break;
        }
        units += character.len_utf16();
        if character == '\n' {
            line += 1;
        }
    }
    line
}

/// Markdown for one new block: a list item as written, or the text made one, its later lines
/// indented under it.
pub fn as_block(text: &str) -> String {
    let value = text.trim();
    static ITEM: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"^[\t\n\x0B\x0C\r ]*(?:[-*+]|[0-9]+[.)])[\t\n\x0B\x0C\r ]")
            .expect("the list item pattern is valid")
    });
    if ITEM.is_match(value) {
        return value.to_owned();
    }
    let mut lines = value.split('\n');
    let first = lines.next().unwrap_or_default();
    std::iter::once(format!("- {first}"))
        .chain(lines.map(|line| {
            if line.is_empty() {
                String::new()
            } else {
                format!("  {line}")
            }
        }))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Refuses a page target that names something outside the notebook's pages.
pub fn outside_pages(value: &str) -> Option<String> {
    let windows_drive = {
        let bytes = value.as_bytes();
        bytes.len() >= 3
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && matches!(bytes[2], b'\\' | b'/')
    };
    let quire = value.len() >= 6
        && value[..6].eq_ignore_ascii_case(".quire")
        && matches!(value.as_bytes().get(6), None | Some(b'/' | b'\\'));
    (value.starts_with('/')
        || windows_drive
        || value.split(['/', '\\']).any(|part| part == "..")
        || quire)
        .then(|| {
            format!(
                "{value} is outside the notebook's pages. Pass a page title or a path inside the notebook."
            )
        })
}

/// A tag as the tools match it: with its leading #.
pub fn hashed(tag: &str) -> String {
    if tag.starts_with('#') {
        tag.to_owned()
    } else {
        format!("#{tag}")
    }
}

/// Removes keys whose value is null, as `JSON.stringify` drops undefined fields.
pub fn compact(value: Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.into_iter()
                .filter(|(_, value)| !value.is_null())
                .map(|(key, value)| (key, compact(value)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.into_iter().map(compact).collect()),
        other => other,
    }
}
