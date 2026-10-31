use crate::json::Json;
use jiff::{
    Span, Timestamp,
    civil::Date,
    tz::{AmbiguousOffset, TimeZone},
};
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

    pub fn folder(&self, iso: &str, calendar: &DailyCalendar) -> Result<String, &'static str> {
        Ok(format_daily(&self.folder, iso, calendar)?
            .trim_matches('/')
            .into())
    }

    pub fn path(&self, iso: &str, calendar: &DailyCalendar) -> Result<String, &'static str> {
        let folder = self.folder(iso, calendar)?;
        let title = format_daily(&self.title, iso, calendar)?;
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

#[derive(Clone)]
pub struct DailyCalendar {
    timezone: TimeZone,
}

impl DailyCalendar {
    pub fn utc() -> Self {
        Self {
            timezone: TimeZone::UTC,
        }
    }

    /// Resolve zone data off the foreground thread, then retain the calendar.
    pub fn named(name: &str) -> Result<Self, String> {
        TimeZone::get(name).map(|timezone| Self { timezone }).map_err(|error| format!("Cannot load time zone {name:?}: {error}. Check the system time zone data and choose a valid zone before opening daily pages."))
    }

    pub fn system() -> Result<Self, String> {
        TimeZone::try_system().map(|timezone| Self { timezone }).map_err(|error| format!("Cannot load the system time zone: {error}. Correct the system time zone settings before opening daily pages."))
    }

    /// The caller supplies the clock instant; no process clock or timezone is read.
    pub fn date_at(&self, unix_milliseconds: i64) -> Result<String, String> {
        let instant = Timestamp::from_millisecond(unix_milliseconds).map_err(|error| format!("Cannot read this daily-page clock value: {error}. Supply an instant in the supported calendar range."))?;
        Ok(date_string(instant.to_zoned(self.timezone.clone()).date()))
    }

    /// The local date, hour, and minute at a clock instant, for template placeholders.
    pub fn clock_at(&self, unix_milliseconds: i64) -> Result<(String, u8, u8), String> {
        let instant = Timestamp::from_millisecond(unix_milliseconds).map_err(|error| format!("Cannot read this clock value: {error}. Supply an instant in the supported calendar range."))?;
        let local = instant.to_zoned(self.timezone.clone()).datetime();
        Ok((
            date_string(local.date()),
            local.hour() as u8,
            local.minute() as u8,
        ))
    }

    fn local_date(&self, year: i32, month: u32, day: u32, hour: i8) -> Option<Date> {
        if !(1..=31).contains(&day) {
            return None;
        }
        let date = civil_date(year, month, 1)?
            .checked_add(Span::new().days(i64::from(day - 1)))
            .ok()?;
        let local = date.at(hour, 0, 0, 0);
        // Compatible disambiguation advances a gap and keeps a fold's civil date.
        // Inspect offsets directly so the last valid four-digit date needs no
        // intermediate timestamp beyond Jiff's narrower timestamp upper bound.
        let local = match self.timezone.to_ambiguous_timestamp(local).offset() {
            AmbiguousOffset::Gap { before, after } => local
                .checked_add(
                    Span::new().seconds(i64::from(after.seconds()) - i64::from(before.seconds())),
                )
                .ok()?,
            _ => local,
        };
        Some(local.date())
    }
}

fn civil_date(year: i32, month: u32, day: u32) -> Option<Date> {
    Date::new(
        year.try_into().ok()?,
        month.try_into().ok()?,
        day.try_into().ok()?,
    )
    .ok()
}

fn date_string(date: Date) -> String {
    format!("{}-{:02}-{:02}", date.year(), date.month(), date.day())
}

/// Formats a validated civil ISO date. Invalid dates return an error, never a path.
pub fn format_daily(
    pattern: &str,
    iso: &str,
    calendar: &DailyCalendar,
) -> Result<String, &'static str> {
    let invalid =
        "Cannot format this daily date. Use an existing calendar date in YYYY-MM-DD form.";
    let (year, month, day) = iso_parts(iso).ok_or(invalid)?;
    civil_date(year, month, day).ok_or(invalid)?;
    // JavaScript's numeric Date constructor interprets years 0..99 as 1900..1999.
    let weekday = calendar
        .local_date(constructor_year(year), month, day, 12)
        .ok_or(invalid)?
        .weekday()
        .to_sunday_zero_offset() as usize;
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

    fn parse(&self, title: &str, calendar: &DailyCalendar) -> Option<String> {
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
        let normalized = calendar.local_date(
            constructor_year(year.parse().ok()?),
            month.parse().ok()?,
            day.parse().ok()?,
            12,
        )?;
        if u32::try_from(normalized.month()).ok()? != month.parse::<u32>().ok()?
            || u32::try_from(normalized.day()).ok()? != day.parse::<u32>().ok()?
        {
            return None;
        }
        let iso = format!("{year}-{month}-{day}");
        (format_daily(&self.source, &iso, calendar)
            .ok()?
            .to_lowercase()
            == title.to_lowercase())
        .then_some(iso)
    }
}

pub fn parse_daily(title: &str, pattern: &str, calendar: &DailyCalendar) -> Option<String> {
    Pattern::new(pattern)?.parse(title, calendar)
}

/// Retain one parser per notebook/layout; configuration is never process-global.
pub struct DailyParser {
    configured: Option<Pattern>,
    calendar: DailyCalendar,
}

impl DailyParser {
    pub fn new(layout: &DailyLayout, calendar: DailyCalendar) -> Self {
        Self {
            calendar,
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
            return ISO_WEEKDAY.parse(title, &self.calendar);
        }
        if let Some(date) = self
            .configured
            .as_ref()
            .and_then(|pattern| pattern.parse(title, &self.calendar))
        {
            return Some(date);
        }
        if let Some((year, month, day)) = iso_parts(title)
            && year >= 1000
            && self
                .calendar
                .local_date(year, month, day, 12)
                .is_some_and(|date| date_string(date) == title)
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
        self.calendar
            .local_date(year, month, day, 0)
            .map(date_string)
    }
}

pub fn daily_date(title: &str, layout: &DailyLayout, calendar: &DailyCalendar) -> Option<String> {
    DailyParser::new(layout, calendar.clone()).date(title)
}

pub fn detect_layout<'a>(
    paths: impl IntoIterator<Item = &'a str>,
    calendar: &DailyCalendar,
) -> Option<DailyLayout> {
    let mut counts = Vec::<(DailyLayout, usize)>::new();
    let mut indices = HashMap::new();
    for path in paths {
        let (folder, title) = path.rsplit_once('/').unwrap_or(("", path));
        let title = crate::markdown::page_name(title);
        for pattern in [&*ISO_WEEKDAY, &*ISO_PATTERN] {
            let Some(date) = pattern.parse(title, calendar) else {
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
