//! The guide shown under the Templates page and the examples it offers, as the reference
//! `template-guide.ts` lists them. The starter templates are four of these examples.

pub struct Placeholder {
    pub token: &'static str,
    pub meaning: &'static str,
}

pub struct Example {
    pub name: &'static str,
    pub usage: &'static str,
    pub body: &'static str,
}

pub const PLACEHOLDERS: [Placeholder; 8] = [
    Placeholder {
        token: "{cursor}",
        meaning: "where the cursor lands after inserting",
    },
    Placeholder {
        token: "{today}",
        meaning: "a link to today's daily page, such as [[2026-10-30]]",
    },
    Placeholder {
        token: "{date: tomorrow}",
        meaning: "a link to another day: tomorrow, yesterday, +3, -1, monday, or 2026-11-02",
    },
    Placeholder {
        token: "{time}",
        meaning: "the time, such as 09:05",
    },
    Placeholder {
        token: "{weekday}",
        meaning: "the day's name, such as Friday",
    },
    Placeholder {
        token: "{page}",
        meaning: "the title of the page you insert into",
    },
    Placeholder {
        token: "{pick: #meeting}",
        meaning: "a fuzzy picker of names used before in blocks with that tag, newest first, or a new name",
    },
    Placeholder {
        token: "{ask: Who?}",
        meaning: "asks you while inserting; the same question is asked once and its answer reused",
    },
];

pub const RULES: [&str; 4] = [
    "Each top-level block on this page that has indented blocks under it is a template. Its text is the name; the indented blocks are what gets inserted.",
    "Type jj at the start of an empty block anywhere, keep typing to narrow the list, and press Enter. In Vim, be in Insert mode first.",
    "Live lists such as {{tasks: next}} and {{query: #idea}}, embeds, tags and links are copied as written, so they work on the page you insert into. Use here in a task list for that page.",
    "Insertion is one undo step. A top-level block without indented blocks is a note, not a template, so you can leave headings or reminders here.",
];

pub const EXAMPLES: [Example; 14] = [
    Example {
        name: "Meeting",
        usage: "Any meeting: pick a past one by name, or create a new one.",
        body: "- [[{pick: #meeting}]] {time} #meeting\n  - Agenda\n    - {cursor}\n  - Notes\n  - Actions",
    },
    Example {
        name: "One-on-one",
        usage: "A recurring conversation; shows what is open with the person.",
        body: "- 1:1 with [[{ask: With whom?}]] #meeting\n  - Their topics\n    - {cursor}\n  - My topics\n  - Open between us\n    - {{tasks: [[{ask: With whom?}]] open}}",
    },
    Example {
        name: "Project",
        usage: "Insert on a project's own page; the tag puts it in Tasks > Projects.",
        body: "- #project\n- Outcome: {cursor}\n- Next actions\n  - {{tasks: here open by:state}}\n- Notes",
    },
    Example {
        name: "Person",
        usage: "Insert on a person's page; tasks that link them gather here.",
        body: "- Role: {cursor}\n- Open with them\n  - {{tasks: here open by:state}}\n- Notes",
    },
    Example {
        name: "My Day",
        usage: "Morning review on the daily page: due, inbox, next, waiting.",
        body: "- Plan for {today}\n  - Due and overdue\n    - {{tasks: due sort:due}}\n  - Inbox to clarify\n    - {{tasks: inbox limit:10}}\n  - Next actions by project\n    - {{tasks: next by:project limit:20}}\n  - Waiting for\n    - {{tasks: waiting}}\n  - Today's three\n    - {cursor}",
    },
    Example {
        name: "Weekly review",
        usage: "The GTD weekly review: empty the inbox, check every list.",
        body: "- Weekly review {today} #review\n  - Inbox to zero\n    - {{tasks: inbox}}\n  - Coming up\n    - {{tasks: upcoming sort:due}}\n  - Every project has a next action\n    - {{tasks: next by:project}}\n  - Waiting for: chase or drop\n    - {{tasks: waiting by:project}}\n  - Someday: promote or let go\n    - {{tasks: someday limit:20}}\n  - Notes\n    - {cursor}",
    },
    Example {
        name: "Shutdown",
        usage: "End of the day: what got done, what moves to tomorrow.",
        body: "- Shutdown {time}\n  - Done today\n    - {{tasks: done here}}\n  - Still open\n    - {{tasks: here open}}\n  - First thing on {date: tomorrow}\n    - {cursor}",
    },
    Example {
        name: "Follow up",
        usage: "A waiting-for task with a check date three days out.",
        body: "- [ ] {ask: Waiting on whom?}: {cursor} #waiting 📅 {date: +3}",
    },
    Example {
        name: "Decision",
        usage: "A short decision record you can link from projects.",
        body: "- Decision: {ask: Decision?} #decision\n  - Context\n    - {cursor}\n  - Options considered\n  - Chosen, and why\n  - Revisit on {date: +90}",
    },
    Example {
        name: "Book",
        usage: "Reading notes on a book's page.",
        body: "- Author: [[{ask: Author?}]] #book\n- Why I'm reading it: {cursor}\n- Highlights\n- What I'll do differently",
    },
    Example {
        name: "Idea",
        usage: "Capture an idea quickly and decide later.",
        body: "- {cursor} #idea\n  - Why it matters\n  - Next step, if any",
    },
    Example {
        name: "Interview",
        usage: "A candidate interview with a fixed set of questions.",
        body: "- Interview: [[{ask: Candidate?}]] {today} #meeting\n  - Role: {ask: Role?}\n  - Questions\n    - Tell me about a hard problem you solved\n    - {cursor}\n  - Signals\n  - Decision",
    },
    Example {
        name: "Incident",
        usage: "A short failure write-up: what broke, impact, fix, follow-ups.",
        body: "- Incident {today} {time}: {ask: What broke?}\n  - Impact\n    - {cursor}\n  - Timeline\n  - Cause\n  - Follow-ups",
    },
    Example {
        name: "Trip",
        usage: "Travel prep with a packing checklist.",
        body: "- Trip to [[{ask: Where?}]] #trip\n  - Bookings\n    - {cursor}\n  - Packing\n    - [ ] Passport\n    - [ ] Chargers",
    },
];

/// The names of the examples a new Templates page starts with.
pub const STARTERS: [&str; 4] = ["Meeting", "Project", "Person", "My Day"];

/// The text that adds an example to the Templates page.
pub fn definition(example: &Example) -> String {
    let body: Vec<String> = example
        .body
        .split('\n')
        .map(|line| format!("  {line}"))
        .collect();
    format!("- {}\n{}\n", example.name, body.join("\n"))
}

/// What the Templates page starts with the first time it opens.
pub fn starter_templates() -> String {
    STARTERS
        .iter()
        .filter_map(|name| EXAMPLES.iter().find(|example| example.name == *name))
        .map(definition)
        .collect()
}
