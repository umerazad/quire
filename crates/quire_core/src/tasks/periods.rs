use super::TaskState;
use jiff::{Span, civil::Date};

fn iso(date: Date) -> String {
    format!("{:04}-{:02}-{:02}", date.year(), date.month(), date.day())
}

/// A strict `YYYY-MM-DD` calendar date.
fn strict_date(text: &str) -> Option<Date> {
    let bytes = text.as_bytes();
    let shaped = bytes.len() == 10
        && bytes.iter().enumerate().all(|(at, byte)| match at {
            4 | 7 => *byte == b'-',
            _ => byte.is_ascii_digit(),
        });
    if !shaped {
        return None;
    }
    let number = |range: std::ops::Range<usize>| text[range].parse::<i16>().ok();
    Date::new(
        number(0..4)?,
        i8::try_from(number(5..7)?).ok()?,
        i8::try_from(number(8..10)?).ok()?,
    )
    .ok()
}

/// What a period may be, for messages.
pub const PERIODS: &str = "today, yesterday, tomorrow, week, last-week, next-week, month, last-month, next-month, year, last-year, next-year, YYYY-MM-DD, YYYY-MM, YYYY, or FROM..TO";

/// A span of days a filter names, relative to a today supplied later: `today`, `yesterday`,
/// `tomorrow`; `week`, `month`, and `year` (this one) with `last-` and `next-` forms, weeks
/// running Monday to Sunday; a `YYYY-MM-DD` day, a `YYYY-MM` month, a `YYYY` year, or an
/// inclusive `FROM..TO` of two dates.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Period {
    /// The day this many days from today.
    Day(i64),
    /// The week, month, or year this many from today's.
    Week(i64),
    Month(i64),
    Year(i64),
    /// Fixed first and last days.
    Dates(String, String),
}

impl Period {
    /// Reads a period word, ignoring case. The error says what was wrong and what to type.
    pub fn parse(word: &str) -> Result<Self, String> {
        let word = word.trim().to_lowercase();
        let relative = |unit: &str| {
            [("last-", -1), ("", 0), ("next-", 1)]
                .into_iter()
                .find_map(|(prefix, step)| (word.strip_prefix(prefix)? == unit).then_some(step))
        };
        Ok(match word.as_str() {
            "" => return Err(format!("A period is missing. Type one of {PERIODS}.")),
            "today" => Self::Day(0),
            "yesterday" => Self::Day(-1),
            "tomorrow" => Self::Day(1),
            _ => {
                if let Some(step) = relative("week") {
                    Self::Week(step)
                } else if let Some(step) = relative("month") {
                    Self::Month(step)
                } else if let Some(step) = relative("year") {
                    Self::Year(step)
                } else if let Some((from, to)) = word.split_once("..") {
                    let (Some(first), Some(last)) = (strict_date(from), strict_date(to)) else {
                        return Err(format!(
                            "\"{word}\" is not a date range. Type two YYYY-MM-DD dates joined by .., such as 2026-10-01..2026-10-15."
                        ));
                    };
                    if first > last {
                        return Err(format!(
                            "\"{word}\" ends before it starts. Put the earlier date first: {to}..{from}."
                        ));
                    }
                    Self::Dates(iso(first), iso(last))
                } else if let Some(day) = strict_date(&word) {
                    Self::Dates(iso(day), iso(day))
                } else if let Some(first) = strict_date(&format!("{word}-01")) {
                    Self::Dates(iso(first), iso(first.last_of_month()))
                } else if let Some(first) = strict_date(&format!("{word}-01-01")) {
                    Self::Dates(iso(first), iso(first.last_of_year()))
                } else {
                    return Err(format!(
                        "\"{word}\" is not a period. Type one of {PERIODS}."
                    ));
                }
            }
        })
    }

    /// The first and last days of the period around `today`, inclusive. None when `today` is
    /// not a `YYYY-MM-DD` date or the period leaves the supported years.
    pub fn bounds(&self, today: &str) -> Option<(String, String)> {
        let now = strict_date(today)?;
        let (first, last) = match self {
            Self::Dates(first, last) => return Some((first.clone(), last.clone())),
            Self::Day(days) => {
                let day = now.checked_add(Span::new().days(*days)).ok()?;
                (day, day)
            }
            Self::Week(weeks) => {
                let back = i64::from(now.weekday().to_monday_zero_offset());
                let monday = now.checked_add(Span::new().days(weeks * 7 - back)).ok()?;
                (monday, monday.checked_add(Span::new().days(6)).ok()?)
            }
            Self::Month(months) => {
                let first = now
                    .first_of_month()
                    .checked_add(Span::new().months(*months))
                    .ok()?;
                (first, first.last_of_month())
            }
            Self::Year(years) => {
                let first = now
                    .first_of_year()
                    .checked_add(Span::new().years(*years))
                    .ok()?;
                (first, first.last_of_year())
            }
        };
        Some((iso(first), iso(last)))
    }
}

/// The inclusive first and last ISO days `word` names around `today`.
pub fn date_range(word: &str, today: &str) -> Result<(String, String), String> {
    Period::parse(word)?.bounds(today).ok_or_else(|| {
        format!("Cannot place \"{word}\" around today ({today}). Type a YYYY-MM-DD date instead.")
    })
}

/// A date term of a task filter: `done:PERIOD` (completed within it), `due:PERIOD` (due within
/// it), `due:overdue` (open and due before today), or `due:none` (no due date).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DateTerm {
    Done(Period),
    Due(Period),
    Overdue,
    NoDue,
}

impl DateTerm {
    /// Reads a `done:` or `due:` term, ignoring case; None when `token` is neither.
    pub fn parse(token: &str) -> Option<Result<Self, String>> {
        let lower = token.to_lowercase();
        if let Some(word) = lower.strip_prefix("done:") {
            return Some(match word {
                "overdue" | "none" => Err(format!(
                    "done:{word} lists nothing: done: takes when a task was completed. Type due:{word}, or done: with one of {PERIODS}."
                )),
                word => Period::parse(word)
                    .map(Self::Done)
                    .map_err(|error| format!("In \"{token}\": {error}")),
            });
        }
        let word = lower.strip_prefix("due:")?;
        Some(match word {
            "overdue" => Ok(Self::Overdue),
            "none" => Ok(Self::NoDue),
            word => Period::parse(word)
                .map(Self::Due)
                .map_err(|error| format!("In \"{token}\": {error} For due: also overdue or none.")),
        })
    }

    /// The term placed around `today`, ready to test tasks. The error says what to type.
    pub fn resolve(&self, today: &str) -> Result<DateTest, String> {
        let bounds = |period: &Period| {
            period.bounds(today).ok_or_else(|| {
                format!("Cannot place a date filter around today ({today}). Retry once the notebook clock reads a date.")
            })
        };
        Ok(match self {
            Self::Done(period) => {
                let (first, last) = bounds(period)?;
                DateTest::Done(first, last)
            }
            Self::Due(period) => {
                let (first, last) = bounds(period)?;
                DateTest::Due(first, last)
            }
            Self::Overdue => DateTest::Overdue(today.to_owned()),
            Self::NoDue => DateTest::NoDue,
        })
    }
}

/// A date term placed around a day.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DateTest {
    /// Done, and completed within the inclusive days.
    Done(String, String),
    /// Due within the inclusive days.
    Due(String, String),
    /// Open and due before the day.
    Overdue(String),
    NoDue,
}

impl DateTest {
    pub fn matches(&self, state: TaskState, due: Option<&str>, completed: Option<&str>) -> bool {
        let within = |day: Option<&str>, first: &str, last: &str| {
            day.is_some_and(|day| first <= day && day <= last)
        };
        match self {
            Self::Done(first, last) => state == TaskState::Done && within(completed, first, last),
            Self::Due(first, last) => within(due, first, last),
            Self::Overdue(today) => {
                state != TaskState::Done && due.is_some_and(|due| due < today.as_str())
            }
            Self::NoDue => due.is_none(),
        }
    }
}
