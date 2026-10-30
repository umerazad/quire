// Checks that the task command extraction reproduces rt's Notebook behavior: each expectation
// below comes from rt tests/tasks.test.ts at 5ea5c22310ff, run through the frozen modules.
import { expect, it } from "vitest";
import { parseBlocks, parseMarkdown, references, tags } from "./markdown/markdown";
import { tasksInPage } from "./core/tasks";
import { captureTask, createdStamp, setTaskDate, taskAt, tasksIn, type Change } from "./core/task-commands";

const today = "2026-10-29";
const made = ` ➕ [[${today}]]`;
const apply = (text: string, changes: Change[]) =>
  [...changes].sort((a, b) => a.from - b.from).reverse().reduce((value, { from, to, insert }) => value.slice(0, from) + insert + value.slice(to), text);
const analyze = (text: string, title = "Page") => {
  const tree = parseMarkdown(text), blocks = parseBlocks(text, tree), refs = references(text, tree);
  return { blocks, tasks: tasksInPage("Page.md", text, blocks, tags(text, tree, refs), refs, tree, title) };
};
const at = (text: string, position: number, action: "toggle" | "cycle", title = "Page") => {
  const { blocks, tasks } = analyze(text, title);
  return taskAt(text, blocks, tasks, title, today, position, action, { anchor: position, head: position });
};
const within = (text: string, from: number, to: number, action: "toggle" | "cycle") => {
  const { blocks, tasks } = analyze(text);
  return apply(text, tasksIn(text, blocks, tasks, "Page", today, from, to, action));
};

it("creates tasks at the cursor, cycles states and reopens Done", () => {
  let text = "- Parent ^parent\n  - Child ^child\n\nParagraph ^paragraph\n";
  text = apply(text, at(text, 4, "toggle").changes);
  expect(text).toBe(`- [ ] Parent${made} ^parent\n  - Child ^child\n\nParagraph ^paragraph\n`);
  for (const state of ["next", "waiting", "someday", "inbox"]) {
    text = apply(text, at(text, 8, "cycle").changes);
    expect(analyze(text).tasks[0].state).toBe(state);
  }
  text = apply(text, at(text, 8, "toggle").changes);
  expect(analyze(text).tasks[0].state).toBe("done");
  text = apply(text, at(text, 8, "cycle").changes);
  expect(analyze(text).tasks[0].state).toBe("inbox");
  expect(apply(text, at(text, text.indexOf("Paragraph"), "toggle").changes)).toContain(`- [ ] Paragraph${made} ^paragraph`);
  expect(apply(text, at(text, text.length, "toggle").changes).endsWith(`\n- [ ] ${made}`)).toBe(true);
});

it.each([
  ["A b\nc d\n", `- [ ] A b${made}\n- [ ] c d${made}\n`],
  ["- A b\n- c d\n", `- [ ] A b${made}\n- [ ] c d${made}\n`],
  ["- A b\n  - c d\n", `- [ ] A b${made}\n  - [ ] c d${made}\n`],
  ["- [ ] A b\n- c d\n", `- [ ] A b\n- [ ] c d${made}\n`],
])("creates one task per selected line in %j", (original, expected) => {
  expect(within(original, 0, original.length - 1, "toggle")).toBe(expected);
});

it("toggles or cycles every selected task and ignores the line after a full-line selection", () => {
  let text = "- [ ] One\n- [ ] Two\n- [ ] Three\n";
  text = within(text, 0, text.indexOf("- [ ] Three"), "toggle");
  expect(analyze(text).tasks.map((task) => task.state)).toEqual(["done", "done", "inbox"]);
  text = within(text, 0, text.indexOf("Two"), "cycle");
  expect(analyze(text).tasks.map((task) => task.state)).toEqual(["inbox", "inbox", "inbox"]);
  text = within(text, 0, text.length, "cycle");
  expect(analyze(text).tasks.map((task) => task.state)).toEqual(["next", "next", "next"]);
});

it("refuses code, headings and front matter", () => {
  const original = "---\ntitle: Example\n\n---\n# Heading\n\n```md\n- sample\n```\n";
  for (const position of ["Example", "\n\n", "Heading", "sample"].map((needle) => original.indexOf(needle)))
    expect(() => at(original, position, "toggle")).toThrow("task cannot be added");
  const nested = "- Notes\n  ```md\n  sample\n  ```\n";
  expect(() => at(nested, nested.indexOf("sample"), "toggle")).toThrow("inside code");
  const plain = "Plain\n\n```md\n- sample\n```\n# Heading\n";
  const { blocks, tasks } = analyze(plain);
  expect(() => tasksIn(plain, blocks, tasks, "Page", today, plain.indexOf("```"), plain.length, "toggle")).toThrow("task cannot be added");
  expect(within(plain, 0, plain.length, "toggle")).toBe(`- [ ] Plain${made}\n\n\`\`\`md\n- sample\n\`\`\`\n# Heading\n`);
});

it("stamps the completion date when done and removes it when reopened", () => {
  let text = "- [ ] Write report #next ^a\n- [ ] Second ^b\n- [ ] Third ^c\n";
  text = apply(text, at(text, 4, "toggle").changes);
  expect(text).toContain("- [x] Write report #next ✅ 2026-10-29 ^a");
  text = apply(text, at(text, 4, "toggle").changes);
  expect(text).toContain("- [ ] Write report #next ^a\n");
});

it("keeps the cursor after an inserted checkbox and out of the new date link", () => {
  expect(at("- Alpha\n- \n\nText\n", 10, "toggle").selection).toEqual({ anchor: 14, head: 14 });
  const text = "- With id ^blk002\n- Plain\n", end = "- With id ^blk002".length;
  expect(at(text, end, "toggle").selection?.head).toBe(`- [ ] With id${made} ^blk002`.length);
  expect(at(text, text.indexOf("Plain"), "toggle").selection?.head).toBe(text.indexOf("Plain") + 4);
});

it("stamps captured and shortcut tasks with today's link, except on today's page", () => {
  expect(captureTask("", "Bring notes", undefined, createdStamp("Inbox", today)).insert).toBe(`- [ ] Bring notes #inbox${made}\n`);
  expect(apply("- Call the bank ^bank\n", at("- Call the bank ^bank\n", 3, "toggle").changes)).toBe(`- [ ] Call the bank${made} ^bank\n`);
  expect(apply("- Written today\n", at("- Written today\n", 3, "toggle", today).changes)).toBe("- [ ] Written today\n");
});

it("sets, changes and clears task dates and refuses a completion date on an open task", () => {
  let text = "- [ ] Plan ➕ [[2026-10-20]] ^plan\n- [x] Done ✅ 2026-10-27\n";
  const task = (index: number) => analyze(text, "Tasks").tasks[index];
  text = apply(text, setTaskDate(task(0), "due", "2026-11-03"));
  expect(text).toContain("- [ ] Plan ➕ [[2026-10-20]] 📅 [[2026-11-03]] ^plan\n");
  text = apply(text, setTaskDate(task(0), "due"));
  expect(text).toContain("- [ ] Plan ➕ [[2026-10-20]] ^plan\n");
  expect(() => setTaskDate(task(0), "completed", "2026-10-28")).toThrow("Complete the task first");
  expect(() => setTaskDate(task(0), "due", "2026-02-30")).toThrow("not a calendar date");
});
