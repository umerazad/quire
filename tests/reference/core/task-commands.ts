// Extracted from rt src/core/store.ts at 5ea5c22310ff: the task command planning in
// nextTaskAction, insideCode, Notebook.currentTask (duplicate ID check), setTaskDate,
// createdStamp, taskAt, tasksIn and captureTask. Each method body is kept; the Notebook state it
// read (the CodeMirror document, cached blocks and tasks, the clock, page lookup) becomes
// parameters, and instead of editing a document each returns the changes it would apply.
// CodeMirror's ChangeSet.mapPos(position, -1) for the two insertions of taskAt is written out.
import type { Tree, SyntaxNode } from "@lezer/common";
import { frontmatterLength, parseMarkdown, tags, type Block } from "../markdown/markdown";
import { dailyDate } from "../daily/workspace";
import { blockAt } from "../index/page-analysis";
import { createdStampFor, taskChanges, taskDateChange, type Task, type TaskDateField, type TaskState } from "./tasks";

export type Change = { from: number; to: number; insert: string };

function insideCode(tree: Tree, position: number) {
  for (let node: SyntaxNode | null = tree.resolveInner(position, -1); node; node = node.parent)
    if (node.name === "FencedCode" || node.name === "CodeBlock") return true;
  return false;
}
export function nextTaskAction(task: Task, action: "toggle" | "cycle") {
  if (action === "toggle") return "toggle";
  const states = ["inbox", "next", "waiting", "someday"] as const;
  return states[(states.indexOf(task.state as (typeof states)[number]) + 1) % states.length];
}
function lineAt(text: string, position: number) {
  const from = text.lastIndexOf("\n", position - 1) + 1,
    end = text.indexOf("\n", position),
    to = end < 0 ? text.length : end;
  return { from, to, text: text.slice(from, to) };
}
function lines(text: string) {
  const result: { from: number; to: number; text: string }[] = [];
  let from = 0;
  for (const line of text.split("\n")) {
    result.push({ from, to: from + line.length, text: line });
    from += line.length + 1;
  }
  return result;
}
export function currentTask(tasks: Task[], task: Task) {
  if (task.id && tasks.filter((value) => value.id === task.id).length !== 1)
    throw new Error("Several tasks share this block ID. Give them distinct IDs in Source before changing their state.");
  const current = tasks.find((value) => (task.id ? value.id === task.id : value.blockKey === task.blockKey));
  if (!current || current.raw !== task.raw) throw new Error("This task changed. Review its current text and try again.");
  return current;
}
export function setTaskDate(task: Task, field: TaskDateField, date?: string) {
  if (date !== undefined && dailyDate(date) !== date)
    throw new Error(`"${date}" is not a calendar date. Choose the date again from the picker.`);
  if (field === "completed" && task.state !== "done")
    throw new Error("Only a done task has a completion date. Complete the task first.");
  return taskDateChange(task, field, date);
}
export function createdStamp(title: string, today: string) {
  return dailyDate(title) === today ? "" : createdStampFor(today);
}
export function taskAt(
  text: string,
  blocks: Block[],
  tasks: Task[],
  title: string,
  today: string,
  position: number,
  action: "toggle" | "cycle",
  selection: { anchor: number; head: number },
): { changes: Change[]; selection?: { anchor: number; head: number } } {
  const line = lineAt(text, position);
  position = Math.max(position, line.to - line.text.trimStart().length);
  if (insideCode(parseMarkdown(text), position))
    throw new Error("A task cannot be added inside code. Move the cursor to a list block or paragraph.");
  const block = blockAt(blocks, position);
  const task = block && tasks.find((value) => value.from === block.contentFrom);
  if (task) return { changes: taskChanges(currentTask(tasks, task), nextTaskAction(task, action), today) };
  if (
    position < frontmatterLength(text) ||
    (block && block.kind !== "list" && block.kind !== "paragraph") ||
    (!block && line.text.trim())
  )
    throw new Error("A task cannot be added here. Place the cursor in a list block, paragraph or empty line.");
  const from = block?.contentFrom ?? line.to,
    insert = block?.kind === "list" ? "[ ] " : "- [ ] ",
    end = block ? block.contentFrom + block.text.length : line.to,
    stamp = createdStamp(title, today),
    mapPos = (value: number) =>
      value + (value > from ? insert.length : 0) + (value > end ? stamp.length : 0),
    map = (value: number) => (value === from ? mapPos(from) + insert.length : mapPos(value));
  return {
    changes: [
      { from, to: from, insert },
      { from: end, to: end, insert: stamp },
    ],
    selection: { anchor: map(selection.anchor), head: map(selection.head) },
  };
}
export function tasksIn(
  text: string,
  blocks: Block[],
  tasks: Task[],
  title: string,
  today: string,
  from: number,
  to: number,
  action: "toggle" | "cycle",
): Change[] {
  const doc = lines(text),
    lineNumber = (position: number) => doc.findIndex((line) => position <= line.to),
    tree = parseMarkdown(text),
    frontmatter = frontmatterLength(text),
    existing: Task[] = [],
    created: { from: number; insert: string }[] = [],
    stamp = createdStamp(title, today);
  let last = lineNumber(to);
  if (to > from && doc[last].from === to) last--;
  for (let number = lineNumber(from); number <= last; number++) {
    const line = doc[number],
      start = line.to - line.text.trimStart().length;
    if (start === line.to || start < frontmatter || insideCode(tree, start)) continue;
    const block = blockAt(blocks, start);
    if (block?.kind === "paragraph")
      created.push(
        { from: start, insert: "- [ ] " },
        { from: line.from + line.text.trimEnd().length, insert: stamp },
      );
    if (block?.kind !== "list" || lineNumber(block.from) !== number) continue;
    const task = tasks.find((value) => value.from === block.contentFrom);
    if (task) existing.push(task);
    else
      created.push(
        { from: block.contentFrom, insert: "[ ] " },
        { from: block.contentFrom + block.text.length, insert: stamp },
      );
  }
  if (!existing.length && !created.length)
    throw new Error("A task cannot be added in this selection. Select list blocks or paragraphs.");
  return created.length
    ? created.map(({ from, insert }) => ({ from, to: from, insert }))
    : existing.flatMap((task) => taskChanges(task, nextTaskAction(task, action), today));
}
export function captureTask(current: string, text: string, link: string | undefined, stamp: string): Change {
  const value = text.trim();
  if (!value || /[\r\n]/.test(value))
    throw new Error("Capture one task at a time. Use its source note for longer details.");
  const status = tags(value).some((tag) =>
    ["#inbox", "#next", "#waiting", "#someday", "#done"].includes(tag.text.toLocaleLowerCase()),
  );
  const placeholder = /^-[ \t]*\n?$/.test(current);
  return {
    from: placeholder ? 0 : current.length,
    to: current.length,
    insert: `${current && !placeholder && !current.endsWith("\n") ? "\n" : ""}- [ ] ${value}${status ? "" : " #inbox"}${link ? ` [[${link}]]` : ""}${stamp}\n`,
  };
}
export type { TaskState };
