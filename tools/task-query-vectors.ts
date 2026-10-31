import { writeFileSync } from "node:fs";
import { stripId } from "../tests/reference/markdown/markdown";
import type { Task, TaskState } from "../tests/reference/core/tasks";
import type { Notebook } from "../tests/reference/core/query-notebook";
import { parseTaskQuery, runTaskQuery, taskQueryLine } from "../tests/reference/core/task-query";

// Runs the frozen rt task query modules over a synthetic notebook. Pages resolve by
// case-insensitive title or path; the Rust test resolves the same way, so the vectors check
// parsing, matching, ordering, limits, and grouping rather than page lookup.
const today = "2026-10-29";
const pages = [
  { path: "Launch.md", title: "Launch", front: [], projectBlock: true },
  { path: "Garden.md", title: "Garden", front: [], projectBlock: true },
  { path: "Projects/Alpha.md", title: "Projects/Alpha", front: [], projectBlock: false },
  { path: "Beta.md", title: "Beta", front: ["project/beta"], projectBlock: false },
  { path: "Gamma.md", title: "Gamma", front: ["Project"], projectBlock: false },
  { path: "Notes.md", title: "Notes", front: ["reading"], projectBlock: false },
  { path: "notes/Ideas.md", title: "notes/Ideas", front: [], projectBlock: false },
  { path: "2026-10-29.md", title: "2026-10-29", front: [], projectBlock: false },
  { path: "Ärger.md", title: "Ärger", front: [], projectBlock: false },
  { path: "apple.md", title: "apple", front: [], projectBlock: false },
  { path: "Zebra.md", title: "Zebra", front: [], projectBlock: true },
];
const words = ["write", "the", "plan", "book", "venue", "Send", "deck", "call", "Ana", "Straße", "über", "tidy", "notes", "file", "taxes", "dig", "beds", "Ship", "ship-it", "a/b", "launch"];
const tagPool = ["#next", "#waiting", "#question", "#learning/go", "#learning", "#doc", "#project/beta", "#ärger", "#a", "#ab", "#work/deep/er"];
const linkPool = ["Launch", "garden", "Projects/Alpha", "Beta", "Missing", "NOTES", "2026-10-29", "Ärger", "notes/ideas"];
const states: TaskState[] = ["inbox", "next", "waiting", "someday", "done"];
const dues = [undefined, undefined, "2026-10-20", "2026-10-29", "2026-10-30", "2026-11-05", "2026-11-06", "2027-01-01"];
let seed = 0x5eed42;
const random = (n: number) => { seed ^= seed << 13; seed ^= seed >>> 17; seed ^= seed << 5; return (seed >>> 0) % n; };
const pick = <T>(pool: readonly T[]) => pool[random(pool.length)];
const some = <T>(pool: readonly T[], most: number) => [...new Set(Array.from({ length: random(most + 1) }, () => pick(pool)))];
const drafted = Array.from({ length: 90 }, (_, index) => {
  const page = pick(pages);
  const text = Array.from({ length: 1 + random(4) }, () => pick(words)).join(" ");
  const tags = some(tagPool, 2).map(tag => tag.toLocaleLowerCase());
  return {
    key: `${page.path}:${index}`,
    path: page.path,
    from: index * 10,
    text: [text, ...tags].join(" "),
    tags,
    links: some(linkPool, 2),
    state: pick(states),
    due: pick(dues),
  };
});
const tasks = drafted.sort((a, b) => a.path.localeCompare(b.path) || a.from - b.from) as unknown as Task[];
const byTitle = (name: string) => {
  const key = name.toLocaleLowerCase();
  return pages.find(page => page.title.toLocaleLowerCase() === key || page.path.replace(/\.md$/, "").toLocaleLowerCase() === key)?.path;
};
const notebook: Notebook = {
  documents: new Map(pages.map(page => [page.path, page])),
  tasks: () => tasks,
  title: path => pages.find(page => page.path === path)?.title ?? path,
  findPage: byTitle,
  taggedBlocks: tag => (tag === "#project" ? pages.filter(page => page.projectBlock).map(page => ({ path: page.path })) : []),
  frontTags: path => pages.find(page => page.path === path)?.front ?? [],
};
const handwritten = [
  "", "  ", "here", "-here", "all", "all here", "open", "next", "next waiting", "done", "due", "upcoming", "inbox someday",
  "#next", "#NEXT", "#learning", "-#learning", "#next -#question", "#", "-#", "#a", "-", "--", "-x",
  "[[Launch]]", "[[launch]]", "[[ Launch ]]", "-[[Launch]]", "[[Missing]]", "[[]]", "[[Projects/Alpha]]", "[[2026-10-29]]", "[[notes/Ideas]]", "[[Ärger]]",
  "project:launch", "project:Alpha", "project:projects/alpha", "project:beta", "-project:launch", "project:", "project:Gamma -project:zebra",
  "deck", "\"the deck\"", "\"THE DECK\"", "-\"the\"", "\"", "\"unclosed", "-\"", "\"\"", "a\"b\"", "straße", "über", "ÜBER",
  "by:page", "by:project", "by:state", "by:STATE", "by:colour", "-by:page", "all by:project", "all by:state", "next by:page",
  "sort:due", "sort:page", "sort:Due", "sort:size", "-sort:due", "all sort:due limit:5",
  "limit:2", "limit:0", "limit:-1", "limit:1.5", "limit:2.0", "limit:1e1", "limit:0x3", "limit:0b11", "limit:0o7", "limit:+4", "limit:", "limit:abc", "limit:1_0", "limit:.5e1", "limit:5.", "limit:Infinity", "limit:1e400", "-limit:2",
  "colour:red", "x:y", "x:", "1:2", "ÄB:c", "https://example.com", "NEXT", "-next", "Here", "HERE all",
  " next　waiting﻿", "next\tby:page", "all [[Launch]] [[garden]]", "all -[[Launch]] -[[Garden]] -#learning",
];
const fragments = ["next", "waiting", "done", "open", "due", "upcoming", "all", "inbox", "someday", "here", "-here", "#next", "-#next", "#learning", "-#waiting", "#a", "[[Launch]]", "-[[Garden]]", "[[Beta]]", "[[NOTES]]", "project:launch", "-project:garden", "project:beta", "project:projects/alpha", "deck", "-deck", "\"the plan\"", "ship", "-ana", "by:page", "by:project", "by:state", "sort:due", "limit:3", "limit:1"];
const seeded = Array.from({ length: 300 }, () => Array.from({ length: 1 + random(4) }, () => pick(fragments)).join(" "));
const queries = [...new Set([...handwritten, ...seeded])].map(source => {
  const parsed = parseTaskQuery(source);
  if ("error" in parsed) return { source, error: parsed.error };
  const result = runTaskQuery(notebook, parsed, "Launch.md", today, tasks);
  return {
    source,
    query: { ...parsed, by: parsed.by ?? null },
    groups: result.groups.map(group => ({ label: group.label, path: group.path ?? null, tasks: group.tasks.map(task => tasks.indexOf(task)) })),
    total: result.total,
  };
});
const lineInputs = [
  "{{tasks}}", "{{tasks: next}}", "{{tasks:next by:page}}", "{{[[tasks]]: #next}}", "{{[[tasks]]}}", "  {{tasks: all}}  ", "{{tasks: next}} ^abc", "{{tasks: next}}^abc",
  "{{tasks: next}} ^abc  ", "{{TASKS: next}}", "{{tasks next}}", "{{tasks: a}b}}", "{{tasks: x}} trailing", "lead {{tasks}}", "{{tasks:}}", "{{tasks: [[Page]]}}",
  " {{tasks}}　", "{{tasks: next}}\t^id-1", "{{query: next}}", "{{[[tasks]]: \"quoted\"}}", "{{tasks}}{{tasks}}", "",
];
const lines = lineInputs.map(line => ({ line, query: taskQueryLine.exec(stripId(line))?.[1] ?? (taskQueryLine.test(stripId(line)) ? "" : null) }));
writeFileSync("tests/fixtures/task-query-vectors.json", JSON.stringify({ today, here: "Launch.md", pages, tasks, queries, lines }) + "\n");
console.log(`Generated ${queries.length} task queries over ${tasks.length} tasks and ${lines.length} query lines from the frozen task query module.`);
