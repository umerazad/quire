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
