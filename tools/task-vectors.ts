import { writeFileSync } from "node:fs";
import type { SyntaxNode } from "@lezer/common";
import { editorText, parseBlocks, parseMarkdown, references, tagExcludedRanges, tags } from "../tests/reference/markdown/markdown";
import { setActiveDailyLayout, type DailyLayout } from "../tests/reference/daily/daily-layout";
import { addDays, isDue, isUpcoming, shortDate, taskDate, taskFilter, tasksInPage, taskChanges, type Task } from "../tests/reference/core/tasks";
import { captureTask, createdStamp, setTaskDate, taskAt, tasksIn } from "../tests/reference/core/task-commands";

const today = "2026-10-29";
const handwritten: [string, string][] = [
  ["Tasks", "- [ ] Plain task ^a\n  - [ ] Child #waiting ^b\n- [x] Finished #next ^c\n- [ ] Deliberate #done ^d\n- {{[[TODO]]}} Roam #next ^e\n- {{DONE}} Old done ^f\n- [ ] Literal `#waiting` [#next](page.md) #someday/maybe ^g\n\n```md\n- [ ] Code example #next\n```\n"],
  ["Page", "- [ ] Send **report** #inbox #meeting ^stable\n"],
  ["Page", "- {{[[DONE]]}} Legacy #done ^roam\n- [ ] Parent\n  continuation #waiting\n  ^parent\n"],
  ["Page", "- Notes for [[Project]] ^parent\n  - [ ] Child action #next ^child\n- [ ] Duplicate ^same\n- [ ] Duplicate ^same\n"],
  ["Page", "- [ ] Legacy #[[waiting]] ^one\n- [ ] Literal `#[[done]]` [#next](https://example.com) ^two\n- [ ] Later #[[someday]] ^three\n"],
  ["Page", "- Parent ^parent\n  - Child ^child\n\nParagraph ^paragraph\n"],
  ["Page", "A b\nc d\n"],
  ["Page", "- A b\n- c d\n"],
  ["Page", "- A b\n  - c d\n"],
  ["Page", "- [ ] A b\n- c d\n"],
  ["Page", "- [ ] One\n- [ ] Two\n- [ ] Three\n"],
  ["Page", "Plain\n\n```md\n- sample\n```\n# Heading\n"],
  ["Page", "---\ntitle: Example\n\n---\n# Heading\n\n```md\n- sample\n```\n"],
  ["Page", "- Notes\n  ```md\n  sample\n  ```\n"],
  ["Tasks", "- [ ] Own [[2026-10-30]] ^own\n- Plan for [[October 29th, 2026]]\n  - [ ] Inherited ^child\n  - [ ] Earlier wins [[2026-10-20]] [[2026-11-05]] ^two\n- [ ] Undated ^none\n- [x] Shipped ✅ 2026-10-27 ^shipped\n"],
  ["2026-10-26", "- [ ] Jotted on a daily page ^daily\n"],
  ["Page", "- [ ] Write report #next ^a\n- [ ] Second ^b\n- [ ] Third ^c\n"],
  ["Page", "- [x] Write report #next ✅ 2026-10-28 ^a\n"],
  ["Page", "- Parent\n  - [ ] Child\n    - Note\n"],
  ["Page", "- Alpha\n- \n\nText\n"],
  ["Tasks", "- [ ] Made ➕ [[2026-10-20]] ^made\n- [ ] Pinned [[2026-10-01]] 📅 [[2026-11-03]] ➕ 2026-10-02 ^pinned\n- [[Launch]] ➕ [[2026-09-01]]\n  - [ ] Under a stamped parent ^child\n"],
  ["Notes", "- Call the bank ^bank\n"],
  ["2026-10-29", "- Written today\n"],
  ["Tasks", "- [ ] Plan ➕ [[2026-10-20]] ^plan\n- [x] Done ✅ 2026-10-27\n"],
  ["Page", "- With id ^blk002\n- Plain\n"],
  ["Page", "- [ ]\n- [x]\n- {{TODO}}\n- [ ]   \n- [X] Upper\n- {{[[DONE]]}}\n"],
  ["Page", "- [ ] مرحبا 🙂 #next ^u1\n  - [ ] اُردُو [[صفحة]] #waiting\n- [x] 中文 ✅ 2026-01-02 #done\n"],
  ["Page", "1. [ ] Numbered #next\n2. [x] Second\n   - [ ] Nested #someday\n"],
  ["Page", "> - [ ] Quoted #next\n> - plain\n"],
  ["Page", "- [ ] Pending tag #pending\n- [ ] #Next upper\n- [ ] #NEXT/sub nested\n- [ ] #waiting #next both\n"],
  ["Page", "- [ ] Tab\tafter #next\t^tabbed\n- [ ]\tTab marker #someday\n"],
  ["Page", "- [ ] Two stamps ✅ 2026-10-01 ✅ 2026-10-02\n- [ ] Bad due 📅 [[2026-02-30]]\n- [ ] Text due 📅 2026-12-01\n"],
  ["Page", "- [x] Done with due 📅 [[2026-10-01]] ✅ 2026-10-05\n- [ ] Overdue 📅 [[2026-10-01]]\n- [ ] Upcoming 📅 [[2026-11-02]]\n"],
  ["Page", "- {{[[TODO]]}}Glued\n- {{TODO]]}} Half\n- {{[[TODO}} Other half\n- [ ]Glued box\n"],
  ["Page", "- Outer [[2026-10-12]] [[Area]]\n  - Middle [[Topic]] ➕ [[2026-10-01]]\n    - [ ] Deep #next\n    - [ ] Own date [[2026-12-25]]\n"],
  ["Page", "* [ ] Star bullet #waiting\n+ [x] Plus bullet\n"],
  ["Page", "- [ ] Code `#next` and [[#waiting]] #someday ^code\n"],
  ["Page", "- [ ] Multi\n  line #next\n  ✅ 2026-01-01\n"],
  ["Page", "Paragraph line one\nline two #next\n\n- [ ] after\n"],
  ["Page", "- [ ] Untitled #next\n- [ ] #waiting\n- [ ] ✅ 2026-10-10\n"],
  ["Page", ""],
  ["Page", "- "],
  ["Page", "No newline at end"],
  ["Page", "- [ ] trailing spaces   \n\n\n- [ ] after blank lines\n"],
  ["Page", "    indented code\n\n- [ ] task\n"],
  ["Page", "| a |\n| - |\n| [ ] cell |\n\n- [ ] after table\n"],
  ["Page", "- [ ] Link [[Page|alias]] ((blockref)) ![[Embed]] #next\n"],
  ["Project/Alpha", "---\ntags: [project/alpha]\n---\n- [ ] Front matter project #next\n"],
];
const fragments = ["- [ ] task", "- [x] done", "- {{[[TODO]]}} roam", "- {{DONE}} old", "- plain", "  - [ ] child", "  - nested", "    - [ ] deep", "Paragraph", "#next", "#waiting", "#[[someday]]", "#done", "#pending", "[[2026-10-30]]", "[[Project]]", "✅ 2026-10-01", "➕ [[2026-10-02]]", "📅 [[2026-10-31]]", "📅 2026-11-01", "`#next`", "^id1", "^id2", "```\n- [ ] code\n```", "# Heading", "> - [ ] quote", "1. [ ] one", "مرحبا🙂", ""];
let state = 0x3f1a2b;
const random = (n: number) => { state ^= state << 13; state ^= state >>> 17; state ^= state << 5; return (state >>> 0) % n; };
const seeded: [string, string][] = Array.from({ length: 120 }, () => {
  const lines = Array.from({ length: 1 + random(6) }, () => {
    const head = fragments[random(9)];
    const tail = Array.from({ length: random(4) }, () => fragments[9 + random(fragments.length - 9)]);
    return [head, ...tail].join(" ").trimEnd();
  });
  const title = ["Page", "2026-10-29", "2026-10-20", "October 29th, 2026"][random(4)];
  return [title, lines.join("\n") + (random(3) ? "\n" : "")];
});
const layouts: DailyLayout[] = [
  { folder: "", title: "{YYYY}-{MM}-{DD}" },
  { folder: "Daily", title: "{MMMM} {DD}, {YYYY}" },
];
const ranges = (text: string) => {
  const tree = parseMarkdown(text), code: [number, number][] = [];
  tree.iterate({ enter(node) { if (node.name === "FencedCode" || node.name === "CodeBlock") { code.push([node.from, node.to]); return false; } } });
  return code;
};
const attempt = <T>(run: () => T) => { try { return { ok: run() }; } catch (error) { return { error: (error as Error).message }; } };
const plain = (task: Task) => ({ ...task, id: task.id ?? null, created: task.created ?? null, due: task.due ?? null, completed: task.completed ?? null });
const lineStarts = (text: string) => [0, ...[...text.matchAll(/\n/g)].map(match => match.index! + 1)];
const cases = layouts.flatMap((layout, group) => [...handwritten, ...seeded].filter((_, index) => group === 0 || index % 3 === 0).map(([title, source], index) => {
  setActiveDailyLayout(layout);
  const text = editorText(source), tree = parseMarkdown(text), blocks = parseBlocks(text, tree), refs = references(text, tree), tokens = tags(text, tree, refs);
  const tasks = tasksInPage("page.md", text, blocks, tokens, refs, tree, title);
  const starts = lineStarts(text);
  const positions = [...new Set(starts.flatMap(start => {
    const end = text.indexOf("\n", start) < 0 ? text.length : text.indexOf("\n", start);
    return [start, Math.min(start + 2, end), end];
  }))];
  const selections = [[0, text.length], ...starts.slice(1).map(start => [0, start]), ...starts.slice(0, -1).map((start, i) => [start, starts[i + 1]])].filter(([from, to]) => from <= to);
  return {
    name: `${group}-${index}`,
    layout,
    title,
    text,
    inputs: {
      blocks: blocks.map(({ key, id, from, to, contentFrom, contentTo, kind, parent, text }) => ({ key, id: id ?? null, from, to, contentFrom, contentTo, kind, parent: parent ?? null, text })),
      references: refs.map(({ kind, syntax, target, from, to, label, media }) => ({ kind, syntax, target, from, to, label: label ?? null, media: media ?? null })),
      tags: tokens,
      syntaxRanges: tagExcludedRanges(text, tree, []),
      codeRanges: ranges(text),
    },
    tasks: tasks.map(plain),
    changes: tasks.map(task => Object.fromEntries((["toggle", "inbox", "next", "waiting", "someday", "done"] as const).map(action => [action, taskChanges(task, action, today)]))),
    dates: tasks.map(task => (["created", "due", "completed"] as const).flatMap(field => [undefined, "2026-11-03", "2026-02-30"].map(date => ({ field, date: date ?? null, result: attempt(() => setTaskDate(task, field, date)) })))),
    stamp: createdStamp(title, today),
    taskAt: positions.flatMap(position => (["toggle", "cycle"] as const).map(action => ({ position, action, result: attempt(() => taskAt(text, blocks, tasks, title, today, position, action, { anchor: position, head: position })) }))),
    tasksIn: selections.flatMap(([from, to]) => (["toggle", "cycle"] as const).map(action => ({ from, to, action, result: attempt(() => tasksIn(text, blocks, tasks, title, today, from, to, action)) }))),
  };
}));
setActiveDailyLayout();
const captures = ["", "  ", "Bring notes", "  Bring notes  ", "Two\nlines", "Review #next", "Review #Next/sub", "Code `#next`", "Review #pending", "مرحبا 🙂", "Ship #done"].flatMap(value =>
  ["", "-", "- ", "-\n", "- \n", "Existing", "Existing\n", "- [ ] one\n"].flatMap(current => [undefined, "Project Alpha"].flatMap(link => ["", " ➕ [[2026-10-29]]"].map(stamp => ({ current, value, valueTags: tags(value), link: link ?? null, stamp, result: attempt(() => captureTask(current, value, link, stamp)) })))));
const dueTask = (due: string | undefined, state = "next") => ({ due, state, text: "", tags: [] }) as unknown as Task;
const dates = {
  addDays: [["2026-10-29", 7], ["2026-12-28", 7], ["2024-02-28", 1], ["2023-02-28", 1], ["2026-03-01", -1], ["2026-01-01", -1], ["2026-10-29", 0], ["2026-10-29", 400]].map(([iso, days]) => ({ iso, days, result: addDays(iso as string, days as number) })),
  upcoming: ["2026-12-27", "2026-12-28", "2026-12-29", "2027-01-04", "2027-01-05", undefined].flatMap(due => ["next", "done", "inbox"].map(state => ({ due: due ?? null, state, today: "2026-12-28", due_now: isDue(dueTask(due, state), "2026-12-28"), upcoming: isUpcoming(dueTask(due, state), "2026-12-28") }))),
  short: [["2026-10-29", "2026-01-01"], ["2027-01-04", "2026-12-28"], ["2026-01-01", "2026-12-31"]].map(([iso, now]) => ({ iso, today: now, result: shortDate(iso, now) })),
  labels: [["done", "2026-10-27", undefined], ["done", undefined, "2026-10-01"], ["next", undefined, "2026-10-01"], ["next", undefined, "2026-10-29"], ["inbox", undefined, "2026-11-02"], ["waiting", undefined, undefined], ["someday", undefined, "2027-02-01"]].map(([state, completed, due]) => ({ state, completed: completed ?? null, due: due ?? null, result: taskDate({ state, completed, due } as unknown as Task, today) ?? null })),
};
const filterTasks = [["Ship invoice #cloudmizan/billing", ["#cloudmizan/billing"]], ["Ship budget #work", ["#work"]], ["Call mom", []], ["Ünïcode Straße #Ärger", ["#ärger"]]] as const;
const filters = ["", "  ", "ship", "ship -#cloudmizan", "-budget", "-daily", "-#", "-", "SHIP budget", "ship  budget", "call -#work", "-#ärger", "daily", "straße"].map(query => ({ query, results: filterTasks.map(([text, tags]) => taskFilter(query)({ text, tags: [...tags] } as unknown as Task, "Daily")) }));
writeFileSync("tests/fixtures/task-vectors.json", JSON.stringify({ today, cases, captures, dates, filters, filterTasks }) + "\n");
console.log(`Generated ${cases.length} task pages, ${captures.length} captures and ${filters.length} filters from the frozen task modules.`);
