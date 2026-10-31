import { runTool } from "/Users/umer/code/rt/src/core/notebook-tools";
import { fixtureNotebook, today } from "/Users/umer/code/rt/tests/fixtures/ai/notebook";

const calls: [string, unknown][] = [
  ["mentions", { page: "Weekly sync" }],
  ["read_page", { page: "today" }],
  ["read_page", { page: "2026-10-21" }],
  ["tasks", { due: "today" }],
  ["tasks", { due: "week" }],
  ["tasks", { due: "overdue" }],
  ["tasks", { state: "open", project: "Projects/Website" }],
  ["tagged_blocks", { tag: "reading", from: "2026-09-28", to: "today" }],
  ["tagged_blocks", { tag: "#reading" }],
  ["tagged_blocks", { tag: "reading", status: "open" }],
  ["search_notebook", { query: "ship v2" }],
  ["list_pages", { namespace: "Projects" }],
  ["list_pages", { daily_from: "2026-10-15", sort: "date" }],
  ["list_pages", { modified_after: "2026-10-26", sort: "modified" }],
  ["list_pages", { prefix: "week" }],
  ["list_pages", {}],
  ["read_block", { id: "^d21" }],
  ["linked_references", { page: "Weekly sync" }],
  ["daily_pages", { from: "2026-10-15", to: "today" }],
  ["read_block", { id: "missing" }],
  ["read_page", { page: "Nope" }],
  ["read_page", { page: "../secret.md" }],
  ["tasks", { due: "range" }],
];
const out = [];
for (const [name, input] of calls) {
  const outcome = await runTool({ notebook: fixtureNotebook(), today }, name, input);
  out.push({ name, input, outcome });
}
console.log(JSON.stringify(out, null, 1));
