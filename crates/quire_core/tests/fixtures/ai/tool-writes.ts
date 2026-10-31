// Reference results of the write tools for the fixture notebook (today 2026-10-28): the staged
// result in Review mode, and in Apply mode the result with the page text it leaves. Run with
// TZ=UTC from the rt checkout: bun run tool-writes.ts > tool-writes.json
import { AiChanges } from "/Users/umer/code/rt/src/core/ai-changes";
import { answerCall } from "/Users/umer/code/rt/src/core/notebook-host";
import { fixtureNotebook, today } from "/Users/umer/code/rt/tests/fixtures/ai/notebook";

const calls: [string, Record<string, unknown>, string[]][] = [
  ["append_block", { page: "today", text: "Notes\nmore" }, ["2026-10-28.md"]],
  ["append_block", { page: "Inbox", text: "- [ ] From the terminal" }, ["Inbox.md"]],
  ["append_block", { page: "Nope", text: "x" }, []],
  ["create_page", { title: "Plan", text: "- a" }, ["Plan.md"]],
  ["create_page", { title: "Inbox" }, []],
  ["replace_block", { id: "d14", text: "[[Weekly sync]] budget is fine ^d14" }, ["2026-10-14.md"]],
  ["replace_block", { id: "^d14", text: "two\nlines" }, []],
  ["replace_block", { key: "L2-1r9n41n", page: "today", text: "[x] Pay rent early ✅ 2026-10-27" }, ["2026-10-28.md"]],
  ["replace_block", { key: "L9-zzz", page: "today", text: "x" }, []],
  ["replace_block", { key: "L1-amisde", text: "x" }, []],
  ["replace_block", { text: "x" }, []],
  ["capture_task", { text: "Buy milk", project: "Projects/Website" }, ["Inbox.md"]],
  ["capture_task", { text: "Two\nlines" }, []],
  ["capture_task", { text: "x", project: "Nope" }, []],
];
const out = [];
for (const [name, input, pages] of calls) {
  const review = fixtureNotebook();
  const staged = await answerCall(
    { notebook: review, today },
    new AiChanges(() => review),
    { name, input, writes: "review" },
  );
  const notebook = fixtureNotebook();
  const applied = await answerCall(
    { notebook, today },
    new AiChanges(() => notebook),
    { name, input, writes: "apply" },
  );
  out.push({
    name,
    input,
    review: staged,
    apply: applied,
    pages: Object.fromEntries(pages.map((path) => [path, notebook.text(path)])),
  });
}
// Task state changes need a task key from the tasks tool.
for (const state of ["done", "waiting"]) {
  const notebook = fixtureNotebook();
  const [task] = (await answerCall({ notebook, today }, new AiChanges(() => notebook), {
    name: "tasks",
    input: { text: "bank" },
    writes: "apply",
  }) as any).result.tasks;
  const applied = await answerCall({ notebook, today }, new AiChanges(() => notebook), {
    name: "set_task_state",
    input: { key: task.key, state },
    writes: "apply",
  });
  out.push({
    name: "set_task_state",
    input: { text: "bank", state },
    apply: applied,
    pages: { "2026-10-28.md": notebook.text("2026-10-28.md") },
  });
}
console.log(JSON.stringify(out, null, 1));
