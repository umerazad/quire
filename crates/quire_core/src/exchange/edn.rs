//! Roam EDN databases, ported from the reference `readEDN`, `ednJSON`, and `importRoamEDN`: a
//! data-only EDN reader that never evaluates reader forms or tag handlers, and the DataScript datoms
//! of a Roam export rebuilt into Roam JSON pages, with detached blocks recovered on their own page.

use super::{
    js_json::from_f64,
    roam::{Imported, RoamMarkdown, import_roam_json},
};
use crate::markdown::js_space;
use regex::Regex;
use serde_json::{Map, Value};
use std::{collections::HashMap, sync::LazyLock};

/// Collections nest at most this deep.
const MAX_DEPTH: usize = 256;
/// The largest integer JavaScript represents exactly.
const MAX_SAFE: f64 = 9_007_199_254_740_991.0;

/// An EDN value as the reference reader returns it to JavaScript.
#[derive(Clone, Debug)]
pub enum Edn {
    Nil,
    Bool(bool),
    Number(f64),
    /// Strings, keywords, and characters, which JavaScript holds alike.
    Text(String),
    /// A number JavaScript cannot hold exactly, or a non-finite literal, kept as written.
    Literal(String),
    Symbol(String),
    Tagged(String, Box<Edn>),
    /// Vectors, lists, and sets.
    List(Vec<Edn>),
    Map(Vec<(Edn, Edn)>),
}

impl Edn {
    /// JavaScript's `SameValueZero` for map keys: primitives by value, objects never equal.
    fn same_key(&self, other: &Edn) -> bool {
        match (self, other) {
            (Edn::Nil, Edn::Nil) => true,
            (Edn::Bool(a), Edn::Bool(b)) => a == b,
            (Edn::Number(a), Edn::Number(b)) => a == b,
            (Edn::Text(a), Edn::Text(b)) => a == b,
            _ => false,
        }
    }

    fn text(&self) -> Option<&str> {
        match self {
            Edn::Text(text) => Some(text),
            _ => None,
        }
    }

    fn get(&self, key: &str) -> Option<&Edn> {
        match self {
            Edn::Map(entries) => entries
                .iter()
                .find(|(entry, _)| entry.text() == Some(key))
                .map(|(_, value)| value),
            _ => None,
        }
    }

    /// The value as a JavaScript safe integer.
    fn safe_integer(&self) -> Option<i64> {
        match self {
            Edn::Number(value) if value.fract() == 0.0 && value.abs() <= MAX_SAFE => {
                Some(*value as i64)
            }
            _ => None,
        }
    }

    /// The reference `ednJSON`.
    pub fn to_json(&self) -> Value {
        let object = |key: &str, value: Value| {
            let mut map = Map::new();
            map.insert(key.into(), value);
            map
        };
        match self {
            Edn::Nil => Value::Null,
            Edn::Bool(value) => Value::Bool(*value),
            Edn::Number(value) => from_f64(*value),
            Edn::Text(text) => Value::String(text.clone()),
            Edn::Literal(text) => Value::Object(object("edn", Value::String(text.clone()))),
            Edn::Symbol(text) => Value::Object(object("symbol", Value::String(text.clone()))),
            Edn::Tagged(tag, value) => {
                let mut map = object("tag", Value::String(tag.clone()));
                map.insert("value".into(), value.to_json());
                Value::Object(map)
            }
            Edn::List(items) => Value::Array(items.iter().map(Edn::to_json).collect()),
            Edn::Map(entries) if entries.iter().all(|(key, _)| key.text().is_some()) => {
                Value::Object(
                    entries
                        .iter()
                        .map(|(key, value)| {
                            (key.text().unwrap_or_default().to_owned(), value.to_json())
                        })
                        .collect(),
                )
            }
            Edn::Map(entries) => Value::Object(object(
                "map",
                Value::Array(
                    entries
                        .iter()
                        .map(|(key, value)| Value::Array(vec![key.to_json(), value.to_json()]))
                        .collect(),
                ),
            )),
        }
    }
}

static TAG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[a-zA-Z][A-Za-z0-9_./-]*$").unwrap());
static NUMBER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[+-]?[0-9]+(?:\.[0-9]+)?(?:[eE][+-]?[0-9]+)?[MNmn]?$").unwrap());
static CHARACTER_CODE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[uU][0-9a-fA-F]{4}$").unwrap());

/// What a read produced: a value, or a `#_` form that reading discards.
enum Read {
    Value(Edn),
    Discarded,
}

struct Reader<'a> {
    text: &'a str,
    /// A byte offset, which runs past the end where the reference's cursor does.
    cursor: usize,
}

impl Reader<'_> {
    fn fail<T>(&self) -> Result<T, String> {
        let within = self.cursor.min(self.text.len());
        let units = self.text[..within].encode_utf16().count() + (self.cursor - within);
        Err(format!(
            "Invalid or unsupported EDN near character {units}. Export the graph again as JSON or EDN and retry."
        ))
    }

    fn peek(&self) -> Option<char> {
        self.text.get(self.cursor..)?.chars().next()
    }

    /// Steps past the next character, or one position past the end.
    fn advance(&mut self) -> Option<char> {
        let next = self.peek();
        self.cursor += next.map_or(1, char::len_utf8);
        next
    }

    fn whitespace(&mut self) {
        while let Some(character) = self.peek() {
            if js_space(character) || character == ',' {
                self.advance();
            } else if character == ';' {
                while self.peek().is_some_and(|character| character != '\n') {
                    self.advance();
                }
            } else {
                break;
            }
        }
    }

    fn token(&mut self) -> Result<&str, String> {
        let start = self.cursor;
        while self
            .peek()
            .is_some_and(|character| !js_space(character) && !",[](){}\";".contains(character))
        {
            self.advance();
        }
        if start == self.cursor {
            return self.fail();
        }
        Ok(&self.text[start..self.cursor])
    }

    fn string(&mut self) -> Result<Edn, String> {
        let mut units: Vec<u16> = Vec::new();
        let mut start = self.cursor;
        while self.cursor < self.text.len() {
            let current = self.advance();
            if current == Some('"') {
                units.extend(self.text[start..self.cursor - 1].encode_utf16());
                return Ok(Edn::Text(String::from_utf16_lossy(&units)));
            }
            if current != Some('\\') {
                continue;
            }
            units.extend(self.text[start..self.cursor - 1].encode_utf16());
            let escaped = self.advance();
            if escaped == Some('u') {
                let hex = self
                    .text
                    .get(self.cursor..self.cursor + 4)
                    .filter(|hex| hex.bytes().all(|byte| byte.is_ascii_hexdigit()));
                let Some(hex) = hex else {
                    return self.fail();
                };
                units.push(u16::from_str_radix(hex, 16).unwrap_or_default());
                self.cursor += 4;
            } else {
                let unit = match escaped {
                    Some('n') => '\n',
                    Some('r') => '\r',
                    Some('t') => '\t',
                    Some('b') => '\u{8}',
                    Some('f') => '\u{c}',
                    Some('"') => '"',
                    Some('\\') => '\\',
                    _ => return self.fail(),
                };
                units.push(unit as u16);
            }
            start = self.cursor;
        }
        self.fail()
    }

    /// Values up to `close`, with discarded forms dropped.
    fn sequence(&mut self, close: char, depth: usize) -> Result<Vec<Edn>, String> {
        let mut values = Vec::new();
        loop {
            self.whitespace();
            if self.peek() == Some(close) {
                self.advance();
                return Ok(values);
            }
            if let Read::Value(value) = self.read(depth + 1)? {
                values.push(value);
            }
        }
    }

    fn read(&mut self, depth: usize) -> Result<Read, String> {
        if depth > MAX_DEPTH {
            return self.fail();
        }
        self.whitespace();
        if self.cursor >= self.text.len() {
            return self.fail();
        }
        let character = self.advance().unwrap_or_default();
        let value = match character {
            '"' => self.string()?,
            '[' | '(' => Edn::List(self.sequence(if character == '[' { ']' } else { ')' }, depth)?),
            '{' => {
                let values = self.sequence('}', depth)?;
                if values.len() % 2 == 1 {
                    return self.fail();
                }
                let mut entries: Vec<(Edn, Edn)> = Vec::with_capacity(values.len() / 2);
                let mut values = values.into_iter();
                while let (Some(key), Some(value)) = (values.next(), values.next()) {
                    if entries.iter().any(|(existing, _)| existing.same_key(&key)) {
                        return self.fail();
                    }
                    entries.push((key, value));
                }
                Edn::Map(entries)
            }
            '#' => match self.peek() {
                Some('#') => {
                    let numeric = self.token()?;
                    if !["#NaN", "#Inf", "#-Inf"].contains(&numeric) {
                        return self.fail();
                    }
                    Edn::Literal(format!("#{numeric}"))
                }
                Some('_') => {
                    self.advance();
                    self.read(depth + 1)?;
                    return Ok(Read::Discarded);
                }
                Some('{') => {
                    self.advance();
                    Edn::List(self.sequence('}', depth)?)
                }
                _ => {
                    let tag = self.token()?.to_owned();
                    if !TAG.is_match(&tag) {
                        return self.fail();
                    }
                    match self.read(depth + 1)? {
                        Read::Value(value) => Edn::Tagged(tag, Box::new(value)),
                        Read::Discarded => return self.fail(),
                    }
                }
            },
            '\\' => {
                let Some(first) = self.advance() else {
                    return self.fail();
                };
                if !first.is_ascii_alphabetic() {
                    return Ok(Read::Value(Edn::Text(first.to_string())));
                }
                self.cursor -= first.len_utf8();
                let name = self.token()?;
                let named = match name {
                    "newline" => Some('\n'),
                    "return" => Some('\r'),
                    "space" => Some(' '),
                    "tab" => Some('\t'),
                    "backspace" => Some('\u{8}'),
                    "formfeed" => Some('\u{c}'),
                    _ => None,
                };
                match named {
                    Some(named) => Edn::Text(named.to_string()),
                    None if CHARACTER_CODE.is_match(name) => Edn::Text(String::from_utf16_lossy(
                        &[u16::from_str_radix(&name[1..], 16).unwrap_or_default()],
                    )),
                    None if name.encode_utf16().count() == 1 => Edn::Text(name.to_owned()),
                    None => return self.fail(),
                }
            }
            _ => {
                self.cursor -= character.len_utf8();
                let atom = self.token()?;
                match atom {
                    "nil" => Edn::Nil,
                    "true" => Edn::Bool(true),
                    "false" => Edn::Bool(false),
                    _ if NUMBER.is_match(atom) => {
                        let digits = atom.strip_suffix(['M', 'N']).unwrap_or(atom);
                        match digits.parse::<f64>() {
                            Ok(number)
                                if number.is_finite()
                                    && (number.fract() != 0.0 || number.abs() <= MAX_SAFE) =>
                            {
                                Edn::Number(number)
                            }
                            _ => Edn::Literal(atom.to_owned()),
                        }
                    }
                    _ if atom.starts_with(':') => Edn::Text(atom.to_owned()),
                    _ if atom.starts_with(|c: char| c.is_ascii_digit() || c == '#') => {
                        return self.fail();
                    }
                    _ => Edn::Symbol(atom.to_owned()),
                }
            }
        };
        Ok(Read::Value(value))
    }
}

/// The reference `readEDN`: one EDN value, refusing anything it cannot read as data.
pub fn read_edn(text: &str) -> Result<Edn, String> {
    let mut reader = Reader { text, cursor: 0 };
    let value = loop {
        if let Read::Value(value) = reader.read(0)? {
            break value;
        }
    };
    reader.whitespace();
    if reader.cursor != text.len() {
        return reader.fail();
    }
    Ok(value)
}

fn invalid<T>(reason: &str) -> Result<T, String> {
    Err(format!(
        "The Roam EDN database has an invalid {reason}. Export the graph again as JSON or EDN and retry."
    ))
}

/// A Roam EDN database rebuilt as Roam JSON pages, with what the rebuild reports.
#[derive(Debug)]
pub struct EdnGraph {
    pub pages: Value,
    datoms: usize,
    detached: usize,
    repaired: usize,
    missing_orders: usize,
}

/// One database entity's attributes, in the order its datoms first set them.
#[derive(Default)]
struct Entity {
    attributes: Vec<(String, Edn)>,
}

impl Entity {
    fn get(&self, key: &str) -> Option<&Edn> {
        self.attributes
            .iter()
            .find(|(attribute, _)| attribute == key)
            .map(|(_, value)| value)
    }

    fn has(&self, key: &str) -> bool {
        self.get(key).is_some()
    }
}

const ALIASES: [(&str, &str); 10] = [
    (":block/uid", "uid"),
    (":block/string", "string"),
    (":node/title", "title"),
    (":block/open", "open"),
    (":block/heading", "heading"),
    (":block/text-align", "text-align"),
    (":children/view-type", "children-view-type"),
    (":block/view-type", "view-type"),
    (":create/time", "create-time"),
    (":edit/time", "edit-time"),
];

struct Database {
    ids: Vec<i64>,
    entities: HashMap<i64, Entity>,
    children: HashMap<i64, Vec<i64>>,
    parent: HashMap<i64, i64>,
    visited: std::collections::HashSet<i64>,
    repaired: usize,
}

impl Database {
    fn entity(&self, id: i64) -> &Entity {
        &self.entities[&id]
    }

    fn attributes(&self, entity: &Entity) -> Map<String, Value> {
        let mut map = Map::new();
        for (key, value) in &entity.attributes {
            if key == ":block/children" {
                continue;
            }
            if key == ":create/user" || key == ":edit/user" {
                let uid = value
                    .safe_integer()
                    .and_then(|user| self.entities.get(&user))
                    .and_then(|user| user.get(":user/uid"))
                    .and_then(Edn::text);
                if let Some(uid) = uid {
                    let mut user = Map::new();
                    user.insert(":user/uid".into(), Value::String(uid.to_owned()));
                    map.insert(key.clone(), Value::Object(user));
                    continue;
                }
            }
            let name = ALIASES
                .iter()
                .find(|(attribute, _)| attribute == key)
                .map_or(key.as_str(), |(_, alias)| alias);
            map.insert(name.to_owned(), value.to_json());
        }
        map
    }

    fn build(&mut self, id: i64, depth: usize) -> Result<Value, String> {
        if depth > 128 || !self.visited.insert(id) {
            return invalid("cyclic, repeated or excessively nested block");
        }
        let entity = self.entity(id);
        if !matches!(entity.get(":block/string"), Some(Edn::Text(_))) {
            return invalid("outline");
        }
        let mut node = self.attributes(entity);
        if node.get("uid").and_then(Value::as_str) == Some("") {
            node.shift_remove("uid");
            self.repaired += 1;
        }
        let children = self.children[&id].clone();
        if !children.is_empty() {
            let built = children
                .into_iter()
                .map(|child| self.build(child, depth + 1))
                .collect::<Result<Vec<_>, _>>()?;
            node.insert("children".into(), Value::Array(built));
        }
        Ok(Value::Object(node))
    }
}

/// The outline part of the reference `importRoamEDN`: the database's pages as Roam JSON pages.
pub fn roam_pages_from_edn(text: &str) -> Result<EdnGraph, String> {
    let parsed = read_edn(text)?;
    let Edn::Tagged(tag, value) = parsed else {
        return invalid("outline");
    };
    if tag != "datascript/DB" || !matches!(*value, Edn::Map(_)) {
        return invalid("outline");
    }
    let (Some(schema @ Edn::Map(_)), Some(Edn::List(datoms))) =
        (value.get(":schema"), value.get(":datoms"))
    else {
        return invalid("outline");
    };
    let mut database = Database {
        ids: Vec::new(),
        entities: HashMap::new(),
        children: HashMap::new(),
        parent: HashMap::new(),
        visited: Default::default(),
        repaired: 0,
    };
    for datom in datoms {
        let Edn::List(parts) = datom else {
            return invalid("datom shape");
        };
        if !(4..=5).contains(&parts.len()) {
            return invalid("datom shape");
        }
        let (Some(id), Some(attribute), Some(_)) = (
            parts[0].safe_integer(),
            parts[1]
                .text()
                .filter(|attribute| attribute.starts_with(':')),
            parts[3].safe_integer(),
        ) else {
            return invalid("datom value");
        };
        if matches!(parts.get(4), Some(Edn::Bool(false))) {
            return invalid("datom value");
        }
        if !database.entities.contains_key(&id) {
            database.ids.push(id);
        }
        let entity = database.entities.entry(id).or_default();
        let many = schema
            .get(attribute)
            .and_then(|declaration| declaration.get(":db/cardinality"))
            .and_then(Edn::text)
            == Some(":db.cardinality/many");
        let value = parts[2].clone();
        match entity
            .attributes
            .iter_mut()
            .find(|(existing, _)| existing == attribute)
        {
            Some((_, Edn::List(values))) if many => values.push(value),
            Some(_) => return invalid("single-valued attribute"),
            None if many => entity
                .attributes
                .push((attribute.to_owned(), Edn::List(vec![value]))),
            None => entity.attributes.push((attribute.to_owned(), value)),
        }
    }
    let mut missing_orders = 0;
    let order = |entity: &Entity| match entity.get(":block/order") {
        Some(Edn::Number(order)) => *order,
        _ => MAX_SAFE,
    };
    for &id in &database.ids {
        let list = match database.entity(id).get(":block/children") {
            None | Some(Edn::Nil) => Vec::new(),
            Some(Edn::List(list)) => list.clone(),
            Some(_) => return invalid("outline"),
        };
        let mut children = Vec::with_capacity(list.len());
        for child in &list {
            let Some(child) = child.safe_integer().filter(|child| {
                database
                    .entities
                    .get(child)
                    .is_some_and(|entity| entity.has(":block/string"))
                    && !database.parent.contains_key(child)
            }) else {
                return invalid("child or repeated parent");
            };
            database.parent.insert(child, id);
            match database.entity(child).get(":block/order") {
                None => missing_orders += 1,
                Some(Edn::Number(order)) if order.is_finite() => {}
                Some(_) => return invalid("outline"),
            }
            children.push(child);
        }
        children.sort_by(|a, b| {
            let (left, right) = (order(database.entity(*a)), order(database.entity(*b)));
            left.partial_cmp(&right)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.cmp(b))
        });
        database.children.insert(id, children);
    }
    let pages: Vec<i64> = database
        .ids
        .iter()
        .copied()
        .filter(|id| database.entity(*id).has(":node/title"))
        .collect();
    let blocks: Vec<i64> = database
        .ids
        .iter()
        .copied()
        .filter(|id| database.entity(*id).has(":block/string"))
        .collect();
    let mut output = Vec::with_capacity(pages.len() + 1);
    for &id in &pages {
        if database.entity(id).has(":block/string") || database.parent.contains_key(&id) {
            return invalid("outline");
        }
        let mut page = database.attributes(database.entity(id));
        let children = database.children[&id]
            .clone()
            .into_iter()
            .map(|child| database.build(child, 0))
            .collect::<Result<Vec<_>, _>>()?;
        page.insert("children".into(), Value::Array(children));
        output.push(Value::Object(page));
    }
    let detached: Vec<i64> = blocks
        .iter()
        .copied()
        .filter(|id| !database.visited.contains(id))
        .collect();
    if !detached.is_empty() {
        let titles: Vec<&str> = pages
            .iter()
            .filter_map(|id| database.entity(*id).get(":node/title").and_then(Edn::text))
            .collect();
        let mut title = "Roam recovered blocks".to_owned();
        let mut suffix = 2;
        while titles.contains(&title.as_str()) {
            title = format!("Roam recovered blocks {suffix}");
            suffix += 1;
        }
        let roots: Vec<i64> = detached
            .iter()
            .copied()
            .filter(|id| {
                database
                    .parent
                    .get(id)
                    .is_none_or(|parent| !database.entity(*parent).has(":block/string"))
            })
            .collect();
        let recovered = roots
            .into_iter()
            .map(|id| database.build(id, 0))
            .collect::<Result<Vec<_>, _>>()?;
        let mut page = Map::new();
        page.insert("title".into(), Value::String(title));
        page.insert("children".into(), Value::Array(recovered));
        output.push(Value::Object(page));
    }
    if database.visited.len() != blocks.len() {
        return invalid(&format!(
            "unreachable outline ({} text blocks)",
            blocks.len() - database.visited.len()
        ));
    }
    Ok(EdnGraph {
        pages: Value::Array(output),
        datoms: datoms.len(),
        detached: detached.len(),
        repaired: database.repaired,
        missing_orders,
    })
}

/// The reference `importRoamEDN`: the rebuilt pages imported as Roam JSON, with the rebuild's
/// report after the first line.
pub fn import_roam_edn(
    mut graph: EdnGraph,
    markdown: &dyn RoamMarkdown,
    new_id: &mut dyn FnMut() -> String,
) -> Result<Imported, String> {
    let lines = graph.report();
    let mut imported = import_roam_json(graph.pages.take(), markdown, new_id)?;
    imported.report.splice(1..1, lines);
    Ok(imported)
}

impl EdnGraph {
    /// The rebuild's report lines, which follow an import report's first line.
    pub fn report(&self) -> Vec<String> {
        let mut lines = vec![format!(
            "Read {} DataScript datoms. Recovered {} detached text blocks on a separate recovery page. The original database archive retains all non-outline entities and settings.",
            self.datoms, self.detached
        )];
        if self.repaired > 0 {
            lines.push(format!(
                "Assigned new identifiers to {} blocks with empty database UIDs; original values remain in the source archive.",
                self.repaired
            ));
        }
        if self.missing_orders > 0 {
            lines.push(format!(
                "{} blocks had no order; placed them after ordered siblings using their database IDs.",
                self.missing_orders
            ));
        }
        lines
    }
}
