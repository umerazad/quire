use crate::json::Json;
use chrono::{Datelike, Days, NaiveDate};
use regex::Regex;
use std::{collections::HashMap, sync::LazyLock};

const DEFAULT_TITLE: &str = "{YYYY}-{MM}-{DD}";
const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];
const WEEKDAYS: [&str; 7] = [
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
];
static TOKENS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\{(YYYY|MMMM|MM|DD|dddd)\}").unwrap());
static ISO: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[0-9]{4}-[0-9]{2}-[0-9]{2}$").unwrap());
static WEEKDAY_TITLE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[0-9]{4}-[0-9]{2}-[0-9]{2}-[A-Za-z]+$").unwrap());
static ROAM: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^([A-Za-z]+) ([0-9]{1,2})(?:st|nd|rd|th)?, ([0-9]{4})$").unwrap()
});
static ISO_WEEKDAY: LazyLock<Pattern> =
    LazyLock::new(|| Pattern::new("{YYYY}-{MM}-{DD}-{dddd}").unwrap());
static ISO_PATTERN: LazyLock<Pattern> = LazyLock::new(|| Pattern::new(DEFAULT_TITLE).unwrap());

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DailyLayout {
    pub folder: String,
    pub title: String,
}

impl Default for DailyLayout {
    fn default() -> Self {
        Self {
            folder: String::new(),
            title: DEFAULT_TITLE.into(),
        }
    }
}

impl DailyLayout {
    pub fn problem(&self) -> Option<&'static str> {
        if !self.title.contains("{YYYY}") || !self.title.contains("{DD}") {
            return Some(
                "The daily title needs {YYYY}, {DD} and {MM} or {MMMM}, such as {YYYY}-{MM}-{DD}.",
            );
        }
        if !self.title.contains("{MM}") && !self.title.contains("{MMMM}") {
            return Some("The daily title needs {MM} or {MMMM} for the month.");
        }
        if self.title.contains(['/', '\\']) {
            return Some(
                "The daily title cannot contain a slash. Put folders in the folder pattern.",
            );
        }
        if self
            .folder
            .split('/')
            .any(|part| part == ".." || part.starts_with('.'))
            || self.folder.starts_with('/')
            || self.folder.contains('\\')
        {
            return Some(
                "The daily folder must be a relative folder inside the notebook, such as Daily/{YYYY}.",
            );
        }
        None
    }

    pub fn folder(&self, iso: &str) -> Result<String, &'static str> {
        Ok(format_daily(&self.folder, iso)?.trim_matches('/').into())
    }

    pub fn path(&self, iso: &str) -> Result<String, &'static str> {
        let folder = self.folder(iso)?;
        let title = format_daily(&self.title, iso)?;
        Ok(format!(
            "{}{title}.md",
            if folder.is_empty() {
                folder
            } else {
                folder + "/"
            }
        ))
    }
}

fn iso_parts(iso: &str) -> Option<(i32, u32, u32)> {
    if !ISO.is_match(iso) {
        return None;
    }
    Some((
        iso[..4].parse().ok()?,
        iso[5..7].parse().ok()?,
        iso[8..].parse().ok()?,
    ))
}

fn constructor_year(year: i32) -> i32 {
    if year <= 99 { year + 1900 } else { year }
}

/// Formats a validated civil ISO date. Invalid dates return an error, never a path.
pub fn format_daily(pattern: &str, iso: &str) -> Result<String, &'static str> {
    let invalid =
        "Cannot format this daily date. Use an existing calendar date in YYYY-MM-DD form.";
    let (year, month, day) = iso_parts(iso).ok_or(invalid)?;
    NaiveDate::from_ymd_opt(year, month, day).ok_or(invalid)?;
    // JavaScript's numeric Date constructor interprets years 0..99 as 1900..1999.
    let weekday = NaiveDate::from_ymd_opt(constructor_year(year), month, 1)
        .and_then(|first| first.checked_add_days(Days::new(u64::from(day - 1))))
        .ok_or(invalid)?
        .weekday()
        .num_days_from_sunday() as usize;
    Ok(TOKENS
        .replace_all(pattern, |capture: &regex::Captures<'_>| match &capture[1] {
            "YYYY" => iso[..4].to_owned(),
            "MM" => iso[5..7].to_owned(),
            "DD" => iso[8..].to_owned(),
            "MMMM" => MONTHS[month as usize - 1].into(),
            "dddd" => WEEKDAYS[weekday].into(),
            _ => unreachable!(),
        })
        .into_owned())
}

// ECMAScript's non-Unicode /i canonicalizes UTF-16 units, excluding expansions
// and non-ASCII letters whose uppercase would become ASCII (for example long s).
fn regex_case(text: &str) -> String {
    text.encode_utf16()
        .map(|unit| {
            let Some(c) = char::from_u32(u32::from(unit)) else {
                return char::from_u32(0xf0000 + u32::from(unit)).unwrap();
            };
            let mut upper = c.to_uppercase();
            let first = upper.next().unwrap();
            if upper.next().is_some()
                || first.len_utf16() != 1
                || (!c.is_ascii() && first.is_ascii())
            {
                c
            } else {
                first
            }
        })
        .collect()
}

struct Pattern {
    source: String,
    names: Vec<String>,
    regex: Regex,
}

impl Pattern {
    fn new(pattern: &str) -> Option<Self> {
        let mut names = Vec::new();
        let mut expression = String::from("^");
        let mut position = 0;
        for token in TOKENS.captures_iter(pattern) {
            let matched = token.get(0).unwrap();
            expression.push_str(&regex::escape(&regex_case(
                &pattern[position..matched.start()],
            )));
            let name = &token[1];
            expression.push_str(match name {
                "YYYY" => "([0-9]{4})",
                "MM" | "DD" => "([0-9]{2})",
                _ => "([A-Z]+)",
            });
            names.push(name.to_owned());
            position = matched.end();
        }
        expression.push_str(&regex::escape(&regex_case(&pattern[position..])));
        expression.push('$');
        Some(Self {
            source: pattern.into(),
            names,
            regex: Regex::new(&expression).ok()?,
        })
    }

    fn parse(&self, title: &str) -> Option<String> {
        let canonical = regex_case(title);
        let captures = self.regex.captures(&canonical)?;
        let value = |name| {
            self.names
                .iter()
                .position(|n| n == name)
                .and_then(|index| captures.get(index + 1))
                .map(|capture| capture.as_str())
        };
        let year = value("YYYY")?;
        let month = value("MM").map(str::to_owned).or_else(|| {
            let name = value("MMMM")?;
            let index = MONTHS
                .iter()
                .position(|month| month.eq_ignore_ascii_case(name))?;
            Some(format!("{:02}", index + 1))
        })?;
        let day = value("DD")?;
        NaiveDate::from_ymd_opt(
            constructor_year(year.parse().ok()?),
            month.parse().ok()?,
            day.parse().ok()?,
        )?;
        let iso = format!("{year}-{month}-{day}");
        (format_daily(&self.source, &iso).ok()?.to_lowercase() == title.to_lowercase())
            .then_some(iso)
    }
}

pub fn parse_daily(title: &str, pattern: &str) -> Option<String> {
    Pattern::new(pattern)?.parse(title)
}

/// Retain one parser per notebook/layout; configuration is never process-global.
pub struct DailyParser {
    configured: Option<Pattern>,
}

impl DailyParser {
    pub fn new(layout: &DailyLayout) -> Self {
        Self {
            configured: (layout.title != DEFAULT_TITLE)
                .then(|| Pattern::new(&layout.title))
                .flatten(),
        }
    }

    pub fn date(&self, title: &str) -> Option<String> {
        if !title
            .as_bytes()
            .windows(4)
            .any(|units| units.iter().all(u8::is_ascii_digit))
        {
            return None;
        }
        if WEEKDAY_TITLE.is_match(title) {
            return ISO_WEEKDAY.parse(title);
        }
        if let Some(date) = self
            .configured
            .as_ref()
            .and_then(|pattern| pattern.parse(title))
        {
            return Some(date);
        }
        if let Some((year, month, day)) = iso_parts(title)
            && year >= 1000
            && NaiveDate::from_ymd_opt(year, month, day).is_some()
        {
            return Some(title.into());
        }
        let captures = ROAM.captures(title)?;
        let name = captures[1].get(..3)?;
        let month = MONTHS
            .iter()
            .position(|month| month[..3].eq_ignore_ascii_case(name))? as u32
            + 1;
        let day: u32 = captures[2].parse().ok()?;
        if !(1..=31).contains(&day) {
            return None;
        }
        let mut year: i32 = captures[3].parse().ok()?;
        if year < 50 {
            year += 2000;
        } else if year < 100 {
            year += 1900;
        }
        let date = NaiveDate::from_ymd_opt(year, month, 1)?
            .checked_add_days(Days::new(u64::from(day - 1)))?;
        Some(format!(
            "{}-{:02}-{:02}",
            date.year(),
            date.month(),
            date.day()
        ))
    }
}

pub fn daily_date(title: &str, layout: &DailyLayout) -> Option<String> {
    DailyParser::new(layout).date(title)
}

pub fn detect_layout<'a>(paths: impl IntoIterator<Item = &'a str>) -> Option<DailyLayout> {
    let mut counts = Vec::<(DailyLayout, usize)>::new();
    let mut indices = HashMap::new();
    for path in paths {
        let (folder, title) = path.rsplit_once('/').unwrap_or(("", path));
        let title = crate::markdown::page_name(title);
        for pattern in [&*ISO_WEEKDAY, &*ISO_PATTERN] {
            let Some(date) = pattern.parse(title) else {
                continue;
            };
            let (_, month, _) = iso_parts(&date).unwrap();
            let folder = folder
                .split('/')
                .map(|part| {
                    let mut part = part
                        .replace(&date[..4], "{YYYY}")
                        .replace(MONTHS[month as usize - 1], "{MMMM}");
                    if let Some(index) = part
                        .match_indices(&date[5..7])
                        .map(|(index, _)| index)
                        .find(|index| {
                            (*index == 0 || !part.as_bytes()[index - 1].is_ascii_digit())
                                && (*index + 2 == part.len()
                                    || !part.as_bytes()[index + 2].is_ascii_digit())
                        })
                    {
                        part.replace_range(index..index + 2, "{MM}");
                    }
                    part
                })
                .collect::<Vec<_>>()
                .join("/");
            let layout = DailyLayout {
                folder,
                title: pattern.source.clone(),
            };
            let next = counts.len();
            let index = *indices.entry(layout.clone()).or_insert_with(|| {
                counts.push((layout, 0));
                next
            });
            counts[index].1 += 1;
            break;
        }
    }
    counts
        .into_iter()
        .enumerate()
        .max_by_key(|(index, (_, count))| (*count, std::cmp::Reverse(*index)))
        .map(|(_, (layout, _))| layout)
}

#[derive(Debug, PartialEq, Eq)]
pub struct NotebookSettings {
    pub layout: DailyLayout,
    pub configured: bool,
    pub read_only: Option<String>,
}

pub fn read_settings(text: Option<&str>) -> NotebookSettings {
    let mut settings = NotebookSettings {
        layout: DailyLayout::default(),
        configured: false,
        read_only: None,
    };
    let Some(text) = text.filter(|text| !text.is_empty()) else {
        return settings;
    };
    let Ok(json) = Json::parse(text.as_bytes()) else {
        settings.read_only = Some(
            ".quire/notebook.json is not valid JSON. Fix or delete it, then reopen the notebook."
                .into(),
        );
        return settings;
    };
    let root = json.root();
    if root.is_null() {
        settings.read_only = Some(
            ".quire/notebook.json is not valid JSON. Fix or delete it, then reopen the notebook."
                .into(),
        );
        return settings;
    }
    if root.field("version").as_f64() != Some(1.) {
        settings.read_only = Some("This notebook's .quire/notebook.json comes from a newer Quire. Daily notes use the default layout; update Quire to use its settings.".into());
        return settings;
    }
    let daily = root.field("daily");
    let folder = daily.field("folder");
    let title = daily.field("title");
    if [folder, title]
        .iter()
        .any(|value| value.is_string() && value.as_str().is_none())
    {
        settings.read_only = Some("This daily layout contains an unpaired UTF-16 surrogate that the native editor cannot display. Preserve .quire/notebook.json and choose a representable layout before changing settings.".into());
        return settings;
    }
    let layout = DailyLayout {
        folder: folder.as_str().unwrap_or_default().into(),
        title: title.as_str().unwrap_or(DEFAULT_TITLE).into(),
    };
    if let Some(problem) = layout.problem() {
        settings.read_only = Some(format!(
            ".quire/notebook.json has an unusable daily layout: {problem}"
        ));
        return settings;
    }
    settings.configured = if daily.is_null() {
        false
    } else if let Some(value) = daily.as_bool() {
        value
    } else if let Some(value) = daily.as_f64() {
        value != 0. && !value.is_nan()
    } else if let Some(value) = daily.as_js_string() {
        !value.is_empty()
    } else {
        true
    };
    settings.layout = layout;
    settings
}

pub fn write_settings(layout: &DailyLayout) -> String {
    format!(
        "{{\n  \"version\": 1,\n  \"daily\": {{\n    \"folder\": {},\n    \"title\": {}\n  }}\n}}\n",
        serde_json::to_string(&layout.folder).unwrap(),
        serde_json::to_string(&layout.title).unwrap()
    )
}
