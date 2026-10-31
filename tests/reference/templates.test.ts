// Checks that the template extraction reproduces rt's behavior: each expectation below comes
// from rt tests/templates.test.ts and tests/fuzzy.test.ts at 5ea5c22310ff, run through the
// frozen modules with a notebook built from the frozen page analysis.
import { expect, it } from "vitest";
import { pageTitle, parseBlocks, references } from "./markdown/markdown";
import { analyzePage, tagKey } from "./index/page-analysis";
import { expandTemplate, pastNames, preserveTemplateRoot, templatesInPage, type PickRequest } from "./core/templates";
import { starterTemplates, templateDefinition, templateExamples } from "./core/template-guide";
import { fuzzyFilter, fuzzyMatch } from "./core/fuzzy";
import type { Notebook } from "./core/template-notebook";

export function notebookOf(documents: Record<string, string>): Notebook & { edit(path: string, text: string): void } {
  const docs = { ...documents };
  return {
    templates: () =>
      Object.entries(docs)
        .flatMap(([path, text]) => templatesInPage(path, text, parseBlocks(text)))
        .sort((a, b) => a.name.localeCompare(b.name)),
    taggedBlocks: (tag) =>
      Object.entries(docs)
        .flatMap(([path, text]) => analyzePage(path, text).tagged.map((item) => ({ ...item, path })))
        .filter((item) => tagKey(item.tag) === tagKey(tag))
        .sort((a, b) => a.path.localeCompare(b.path) || a.from - b.from),
    title: (path) => pageTitle(path, docs[path] ?? ""),
    edit: (path, text) => {
      docs[path] = text;
    },
  };
}

const setup = (text: string, extra: Record<string, string> = {}) =>
  notebookOf({ "Templates.md": text, "2026-10-27.md": "- Target ^target", ...extra });
const now = new Date(2026, 9, 30, 9, 5);
const insert = (notebook: Notebook, name: string, path = "2026-10-27.md", prompt: (question: string) => string | null = () => "Ana") =>
  expandTemplate(notebook, notebook.templates().find((item) => item.name === name)!, path, { now, prompt });

it("makes every top-level block with children on the Templates page a template", () => {
  const notebook = setup("- Meeting\n  - Agenda\n- A note without children\n- **Bold** name\n  - Body\n    - Nested\n", { "Other.md": "- Not here\n  - Body\n" });
  expect(notebook.templates().map((item) => [item.name, item.body])).toEqual([
    ["Bold name", "- Body\n  - Nested"],
    ["Meeting", "- Agenda"],
  ]);
});

it("fills placeholders and puts the cursor at {cursor}", async () => {
  const notebook = setup("- Meeting\n  - [[{ask: Meeting with?}]] {time} on {weekday}\n    - Agenda\n      - {cursor}\n    - Again [[{ask: Meeting with?}]] for {page}\n");
  const questions: string[] = [];
  const result = await insert(notebook, "Meeting", "2026-10-27.md", (q) => {
    questions.push(q);
    return "Ana";
  });
  expect(questions).toEqual(["Meeting with?"]);
  expect(result.text).toBe("- [[Ana]] 09:05 on Friday\n  - Agenda\n    - \n  - Again [[Ana]] for 2026-10-27\n");
  expect(result.text.slice(0, result.cursor)).toBe("- [[Ana]] 09:05 on Friday\n  - Agenda\n    - ");
});

it("writes dates as links to daily pages", async () => {
  const notebook = setup("- Dates\n  - {today} {date: tomorrow} {date:-1} {date: +3} {date: monday} {date: fri} {date: 2027-01-02}\n");
  expect((await insert(notebook, "Dates")).text).toBe("- [[2026-10-30]] [[2026-10-31]] [[2026-10-29]] [[2026-11-02]] [[2026-11-02]] [[2026-11-06]] [[2027-01-02]]\n");
  await expect(insert(setup("- Bad\n  - {date: someday}\n"), "Bad")).rejects.toThrow(/not understood/);
});

it("copies live lists, cancels, and gives fresh ids", async () => {
  expect((await insert(setup("- Review\n  - {{tasks: here open by:state}}\n  - {{query: #next}} {unknown} {{today}}\n"), "Review")).text).toBe(
    "- {{tasks: here open by:state}}\n- {{query: #next}} {unknown} {{today}}\n",
  );
  await expect(insert(setup("- Ask\n  - {ask: Who?}\n"), "Ask", "2026-10-27.md", () => null)).rejects.toThrow(/cancelled/);
  const { text } = await insert(setup("- Linked\n  - First ^one\n  - See ((one))\n  - Plain ^two\n"), "Linked"),
    id = /First \^([\w-]+)/.exec(text)?.[1];
  expect(id).not.toBe("one");
  expect(text).toBe(`- First ^${id}\n- See ((${id}))\n- Plain\n`);
});

it("refuses a changed template and keeps the trigger block's id", async () => {
  const notebook = setup("- Meeting\n  - Agenda\n");
  const listed = notebook.templates()[0];
  notebook.edit("Templates.md", "- Meeting\n  - Agenda\n  - More\n");
  await expect(expandTemplate(notebook, listed, "2026-10-27.md")).rejects.toThrow(/changed/);
  const text = preserveTemplateRoot("- First ^old\n- See ((old))\n", "target");
  expect(text).toBe("- First ^target\n- See ((target))\n");
  expect(parseBlocks(text)[0].id).toBe("target");
  expect(references(text).map((ref) => ref.target)).toEqual(["target"]);
});

it("expands every starter and example with no placeholder left", async () => {
  const starters = setup(starterTemplates);
  expect(starters.templates().map((item) => item.name)).toEqual(["Meeting", "My Day", "Person", "Project"]);
  const examples = setup(templateExamples.map(templateDefinition).join(""));
  expect(examples.templates()).toHaveLength(14);
  for (const item of examples.templates())
    expect((await insert(examples, item.name)).text).not.toMatch(/\{(cursor|ask|today|date|time|page|weekday)\b/);
});

it("collects past names and fills {pick}", async () => {
  const notebook = setup("- Meeting\n  - [[{pick: #meeting}]] #meeting\n    - Again {pick: meeting}\n", {
    "2026-10-20.md": "- [[Ops review]] 10:00 #meeting\n- [[Team Sync]] #meeting\n",
    "2026-10-28.md": "- [[Team Sync]] #meeting\n- [[2026-10-28]] [[Design crit|crit]] #meeting\n",
    "2026-10-29.md": "- [[team sync]] #meeting\n- #meeting with nobody named\n",
    "Board prep.md": "- #meeting\n- notes\n",
    "Untagged.md": "- [[Not a meeting]]\n",
  });
  expect(pastNames(notebook, "#meeting")).toEqual([
    { name: "Team Sync", uses: 3, last: "2026-10-29" },
    { name: "Design crit", uses: 1, last: "2026-10-28" },
    { name: "Ops review", uses: 1, last: "2026-10-20" },
    { name: "Board prep", uses: 1, last: "" },
  ]);
  const requests: PickRequest[] = [];
  const picked = await expandTemplate(notebook, notebook.templates()[0], "2026-10-27.md", {
    now,
    pick: (request) => {
      requests.push(request);
      return "Team Sync";
    },
  });
  expect(picked.text).toBe("- [[Team Sync]] #meeting\n  - Again Team Sync\n");
  expect(requests.map((request) => request.tag)).toEqual(["#meeting"]);
});

it("ranks fuzzy matches as rt does", () => {
  const names = ["Tests", "Team Sync", "Ops review", "Ops", "Quarterly planning", "Design crit"];
  const rank = (query: string) => fuzzyFilter(names, query, (name) => name).map(({ item }) => item);
  expect(rank("ts")[0]).toBe("Team Sync");
  expect(rank("ops")).toEqual(["Ops", "Ops review"]);
  expect(rank("")).toEqual(names);
  expect(fuzzyMatch("rv", "🚀 Launch review")?.positions).toEqual([9, 11]);
});
