// The guide shown under the Templates page, and the examples it offers.
export type TemplatePlaceholder = { token: string; meaning: string };
export type TemplateExample = {
  name: string;
  use: string;
  body: string;
};

export const templatePlaceholders: TemplatePlaceholder[] = [
  { token: "{cursor}", meaning: "where the cursor lands after inserting" },
  {
    token: "{today}",
    meaning: "a link to today's daily page, such as [[2026-10-30]]",
  },
  {
    token: "{date: tomorrow}",
    meaning:
      "a link to another day: tomorrow, yesterday, +3, -1, monday, or 2026-11-02",
  },
  { token: "{time}", meaning: "the time, such as 09:05" },
  { token: "{weekday}", meaning: "the day's name, such as Friday" },
  { token: "{page}", meaning: "the title of the page you insert into" },
  {
    token: "{pick: #meeting}",
    meaning:
      "a fuzzy picker of names used before in blocks with that tag, newest first, or a new name",
  },
  {
    token: "{ask: Who?}",
    meaning:
      "asks you while inserting; the same question is asked once and its answer reused",
  },
];

export const templateRules = [
  "Each top-level block on this page that has indented blocks under it is a template. Its text is the name; the indented blocks are what gets inserted.",
  "Type jj at the start of an empty block anywhere, keep typing to narrow the list, and press Enter. In Vim, be in Insert mode first.",
  "Live lists such as {{tasks: next}} and {{query: #idea}}, embeds, tags and links are copied as written, so they work on the page you insert into. Use here in a task list for that page.",
  "Insertion is one undo step. A top-level block without indented blocks is a note, not a template, so you can leave headings or reminders here.",
];

const lines = (...rows: string[]) => rows.join("\n");

export const templateExamples: TemplateExample[] = [
  {
    name: "Meeting",
    use: "Any meeting: pick a past one by name, or create a new one.",
    body: lines(
      "- [[{pick: #meeting}]] {time} #meeting",
      "  - Agenda",
      "    - {cursor}",
      "  - Notes",
      "  - Actions",
    ),
  },
  {
    name: "One-on-one",
    use: "A recurring conversation; shows what is open with the person.",
    body: lines(
      "- 1:1 with [[{ask: With whom?}]] #meeting",
      "  - Their topics",
      "    - {cursor}",
      "  - My topics",
      "  - Open between us",
      "    - {{tasks: [[{ask: With whom?}]] open}}",
    ),
  },
  {
    name: "Project",
    use: "Insert on a project's own page; the tag puts it in Tasks > Projects.",
    body: lines(
      "- #project",
      "- Outcome: {cursor}",
      "- Next actions",
      "  - {{tasks: here open by:state}}",
      "- Notes",
    ),
  },
  {
    name: "Person",
    use: "Insert on a person's page; tasks that link them gather here.",
    body: lines(
      "- Role: {cursor}",
      "- Open with them",
      "  - {{tasks: here open by:state}}",
      "- Notes",
    ),
  },
  {
    name: "My Day",
    use: "Morning review on the daily page: due, inbox, next, waiting.",
    body: lines(
      "- Plan for {today}",
      "  - Due and overdue",
      "    - {{tasks: due sort:due}}",
      "  - Inbox to clarify",
      "    - {{tasks: inbox limit:10}}",
      "  - Next actions by project",
      "    - {{tasks: next by:project limit:20}}",
      "  - Waiting for",
      "    - {{tasks: waiting}}",
      "  - Today's three",
      "    - {cursor}",
    ),
  },
  {
    name: "Weekly review",
    use: "The GTD weekly review: empty the inbox, check every list.",
    body: lines(
      "- Weekly review {today} #review",
      "  - Inbox to zero",
      "    - {{tasks: inbox}}",
      "  - Coming up",
      "    - {{tasks: upcoming sort:due}}",
      "  - Every project has a next action",
      "    - {{tasks: next by:project}}",
      "  - Waiting for: chase or drop",
      "    - {{tasks: waiting by:project}}",
      "  - Someday: promote or let go",
      "    - {{tasks: someday limit:20}}",
      "  - Notes",
      "    - {cursor}",
    ),
  },
  {
    name: "Shutdown",
    use: "End of the day: what got done, what moves to tomorrow.",
    body: lines(
      "- Shutdown {time}",
      "  - Done today",
      "    - {{tasks: done here}}",
      "  - Still open",
      "    - {{tasks: here open}}",
      "  - First thing on {date: tomorrow}",
      "    - {cursor}",
    ),
  },
  {
    name: "Follow up",
    use: "A waiting-for task with a check date three days out.",
    body: lines(
      "- [ ] {ask: Waiting on whom?}: {cursor} #waiting 📅 {date: +3}",
    ),
  },
  {
    name: "Decision",
    use: "A short decision record you can link from projects.",
    body: lines(
      "- Decision: {ask: Decision?} #decision",
      "  - Context",
      "    - {cursor}",
      "  - Options considered",
      "  - Chosen, and why",
      "  - Revisit on {date: +90}",
    ),
  },
  {
    name: "Book",
    use: "Reading notes on a book's page.",
    body: lines(
      "- Author: [[{ask: Author?}]] #book",
      "- Why I'm reading it: {cursor}",
      "- Highlights",
      "- What I'll do differently",
    ),
  },
  {
    name: "Idea",
    use: "Capture an idea quickly and decide later.",
    body: lines(
      "- {cursor} #idea",
      "  - Why it matters",
      "  - Next step, if any",
    ),
  },
  {
    name: "Interview",
    use: "A candidate interview with a fixed set of questions.",
    body: lines(
      "- Interview: [[{ask: Candidate?}]] {today} #meeting",
      "  - Role: {ask: Role?}",
      "  - Questions",
      "    - Tell me about a hard problem you solved",
      "    - {cursor}",
      "  - Signals",
      "  - Decision",
    ),
  },
  {
    name: "Incident",
    use: "A short failure write-up: what broke, impact, fix, follow-ups.",
    body: lines(
      "- Incident {today} {time}: {ask: What broke?}",
      "  - Impact",
      "    - {cursor}",
      "  - Timeline",
      "  - Cause",
      "  - Follow-ups",
    ),
  },
  {
    name: "Trip",
    use: "Travel prep with a packing checklist.",
    body: lines(
      "- Trip to [[{ask: Where?}]] #trip",
      "  - Bookings",
      "    - {cursor}",
      "  - Packing",
      "    - [ ] Passport",
      "    - [ ] Chargers",
    ),
  },
];

// The text that adds an example to the Templates page.
export const templateDefinition = (example: TemplateExample) =>
  `- ${example.name}\n${example.body
    .split("\n")
    .map((line) => `  ${line}`)
    .join("\n")}\n`;

// What the Templates page starts with the first time it opens.
export const starterTemplates = ["Meeting", "Project", "Person", "My Day"]
  .map((name) =>
    templateDefinition(templateExamples.find((item) => item.name === name)!),
  )
  .join("");
