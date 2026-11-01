use super::{Task, TaskState};
use jiff::{Span, civil::Date};

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

fn parts(iso: &str) -> Option<(i16, i8, i8)> {
    let mut parts = iso.splitn(3, '-').map(str::parse::<i64>);
    let (year, month, day) = (
        parts.next()?.ok()?,
        parts.next()?.ok()?,
        parts.next()?.ok()?,
    );
    Some((
        year.try_into().ok()?,
        month.try_into().ok()?,
        day.try_into().ok()?,
    ))
}

/// The ISO date `days` after `iso`; out-of-range days roll into the next month, as JavaScript
/// dates do. None when `iso` is not a date in the supported range.
pub fn add_days(iso: &str, days: i64) -> Option<String> {
    let (year, month, day) = parts(iso)?;
    let date = Date::new(year, month, 1)
        .ok()?
        .checked_add(Span::new().days(i64::from(day) - 1 + days))
        .ok()?;
    Some(format!(
        "{:04}-{:02}-{:02}",
        date.year(),
        date.month(),
        date.day()
    ))
}

/// The Monday after `today`: tomorrow on a Sunday, a week ahead on a Monday. None when `today`
/// is not a date in the supported range.
pub fn next_week(today: &str) -> Option<String> {
    let (year, month, day) = parts(today)?;
    let weekday = Date::new(year, month, day).ok()?.weekday();
    add_days(today, 7 - i64::from(weekday.to_monday_zero_offset()))
}

/// Open with a due date today or earlier.
pub fn is_due(task: &Task, today: &str) -> bool {
    task.state != TaskState::Done && task.due.as_deref().is_some_and(|due| due <= today)
}

/// Open and due after today, within the next `days` days.
pub fn is_upcoming(task: &Task, today: &str, days: i64) -> bool {
    task.state != TaskState::Done
        && task.due.as_deref().is_some_and(|due| {
            due > today && add_days(today, days).is_some_and(|until| due <= until.as_str())
        })
}

/// `Oct 29`, with the year when it is not this year's.
pub fn short_date(iso: &str, today: &str) -> String {
    let Some((year, month, day)) = parts(iso) else {
        return iso.to_owned();
    };
    let name = MONTHS.get(month as usize - 1).copied().unwrap_or_default();
    if iso.get(..4) == today.get(..4) {
        format!("{name} {day}")
    } else {
        format!("{name} {day}, {year}")
    }
}

const WEEKDAYS: [&str; 7] = [
    "monday",
    "tuesday",
    "wednesday",
    "thursday",
    "friday",
    "saturday",
    "sunday",
];

/// What a typed date means: empty clears the date; `today`, `tomorrow`, `yesterday`, `+N` and
/// `-N` days, and a weekday name or its first three letters (the next such day after today)
/// are relative to `today`; anything else must be a YYYY-MM-DD date.
pub fn date_phrase(phrase: &str, today: &str) -> Result<Option<String>, String> {
    let phrase = phrase.trim().to_lowercase();
    let offset = match phrase.as_str() {
        "" => return Ok(None),
        "today" => Some(0),
        "tomorrow" => Some(1),
        "yesterday" => Some(-1),
        signed if signed.starts_with(['+', '-']) => signed
            .parse::<i64>()
            .ok()
            .filter(|days| days.abs() <= 36_600),
        name => WEEKDAYS
            .iter()
            .position(|day| name.len() >= 3 && day.starts_with(name))
            .and_then(|target| {
                let (year, month, day) = parts(today)?;
                let current = Date::new(year, month, day).ok()?.weekday();
                let ahead =
                    (target as i64 - i64::from(current.to_monday_zero_offset())).rem_euclid(7);
                Some(if ahead == 0 { 7 } else { ahead })
            }),
    };
    match offset {
        Some(days) => add_days(today, days).map(Some).ok_or_else(|| {
            format!("Cannot count {days} days from {today}. Type a YYYY-MM-DD date instead.")
        }),
        None if parts(&phrase).is_some() => Ok(Some(phrase)),
        None => Err(format!(
            "\"{phrase}\" is not a date. Type YYYY-MM-DD, today, tomorrow, +3, or a weekday."
        )),
    }
}

/// The task row's date label: when it was done, or when it is due.
pub fn task_date_label(task: &Task, today: &str) -> Option<String> {
    if task.state == TaskState::Done {
        return task
            .completed
            .as_deref()
            .map(|completed| format!("Done {}", short_date(completed, today)));
    }
    let due = task.due.as_deref()?;
    Some(match due.cmp(today) {
        std::cmp::Ordering::Less => format!("Overdue · {}", short_date(due, today)),
        std::cmp::Ordering::Equal => "Today".into(),
        std::cmp::Ordering::Greater => format!("Due {}", short_date(due, today)),
    })
}
