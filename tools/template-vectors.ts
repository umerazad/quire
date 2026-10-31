import { writeFileSync } from "node:fs";
import { copyWithNewIds, editorText, pageTitle, parseBlocks, parseMarkdown, references, type Block } from "../tests/reference/markdown/markdown";
import { analyzePage, tagKey } from "../tests/reference/index/page-analysis";
import { setActiveDailyLayout, type DailyLayout } from "../tests/reference/daily/daily-layout";
import { expandTemplate, pastNames, preserveTemplateRoot, templateDate, templatesInPage, type PickRequest } from "../tests/reference/core/templates";
import { starterTemplates, templateDefinition, templateExamples, templatePlaceholders, templateRules } from "../tests/reference/core/template-guide";
import { fuzzyFilter, fuzzyMatch } from "../tests/reference/core/fuzzy";
import type { Notebook } from "../tests/reference/core/template-notebook";

// Runs the frozen rt template, guide, and fuzzy modules. Block IDs come from a counter in place
// of crypto.randomUUID, so the Rust test generates the same sequence: b-00000000000040008000
// followed by the counter as twelve hex digits, restarting at 1 for every copy.
process.env.TZ = "UTC";
let counter = 0;
Object.defineProperty(globalThis.crypto, "randomUUID", {
  configurable: true,
  value: () => `00000000-0000-4000-8000-${(++counter).toString(16).padStart(12, "0")}`,
});

const blocksJson = (blocks: Block[]) => blocks.map(({ key, id, kind, from, contentFrom, contentTo, subtreeTo, parent, text }) => ({ key, id: id ?? null, kind, from, contentFrom, contentTo, subtreeTo, parent: parent ?? null, text }));
const refsJson = (text: string) => references(text, parseMarkdown(text)).map(({ kind, syntax, target, from, to, label, media }) => ({ kind, syntax, target, from, to, label: label ?? null, media: media ?? null }));
const parsed = (text: string) => ({ blocks: blocksJson(parseBlocks(text)), references: refsJson(text) });

function notebookOf(docs: Record<string, string>): Notebook {
  return {
    templates: () => Object.entries(docs).flatMap(([path, text]) => templatesInPage(path, text, parseBlocks(text))).sort((a, b) => a.name.localeCompare(b.name)),
    taggedBlocks: tag => Object.entries(docs)
      .flatMap(([path, text]) => analyzePage(path, text).tagged.map(item => ({ ...item, path })))
      .filter(item => tagKey(item.tag) === tagKey(tag))
      .sort((a, b) => a.path.localeCompare(b.path) || a.from - b.from),
    title: path => pageTitle(path, docs[path] ?? ""),
  };
}

const lines = (...rows: string[]) => rows.join("\n") + "\n";
const handwritten: string[] = [
  lines("- Meeting", "  - Agenda", "- A note without children", "- **Bold** name", "  - Body", "    - Nested"),
  lines("- Meeting", "  - [[{ask: Meeting with?}]] {time} on {weekday}", "    - Agenda", "      - {cursor}", "    - Again [[{ask: Meeting with?}]] for {page}"),
  lines("- Dates", "  - {today} {date: tomorrow} {date:-1} {date: +3} {date: monday} {date: fri} {date: 2027-01-02}"),
  lines("- Bad", "  - {date: someday}"),
  lines("- Review", "  - {{tasks: here open by:state}}", "  - {{query: #next}} {unknown} {{today}}"),
  lines("- Ask", "  - {ask: Who?}"),
  lines("- Linked", "  - First ^one", "  - See ((one))", "  - Plain ^two"),
  lines("- Scoped", "  - First ^one", "  - See [[Templates#^one]] and {{embed: ((one))}} and ![[Templates#^one]]", "  - Other [[Elsewhere#^one]] ((missing))"),
  lines("- Root referenced", "  - Head ^head", "    - Child ((head))", "  - Tail"),
  lines("- Multi", "  - First line", "    second line ^multi", "  - Link ((multi))", "  - Lone ^lone", "  - ((lone))"),
  lines("- Duplicate", "  - One ^dup", "  - Two ^dup", "  - See ((dup))"),
  lines("- Cases", "  - {TODAY} {Date: Tomorrow} {CURSOR} {Time} {WEEKDAY} {Page}", "  - {{today}} {today}} {{today} {today:} {cursor:x}", "  - {ask} {ask:} {ask: Who?} {ask:Who?} {ASK: Who?}"),
  lines("- Picks", "  - [[{pick: #meeting}]] #meeting", "    - Again {pick: meeting} {pick} {pick: #Meeting} {ask: #meeting}"),
  lines("- Phrases", "  - {date: today} {date:} {date:   } {date: TOMORROW} {date: Yesterday} {date: -0} {date: +0} {date: +9999} {date: -30}", "  - {date: sun} {date: sunday} {date: tue} {date: SAT} {date: 2026-02-30} {date: wed}"),
  lines("- Short weekday", "  - {date: su}"),
  lines("- Too far", "  - {date: +10000}"),
  lines("- Spaced", "  - {date: 1 week}"),
  lines("- Cursors", "  - First {cursor}", "  - Second {cursor}"),
  lines("- No cursor", "  - Plain", "    - Deep"),
  lines("- Nested cursor", "  - Top", "    - Middle", "      - {cursor} after"),
  lines("- `Code` _name_ *x*", "  - body"),
  lines("- Tabs\tand  spaces   ", "  - body"),
  lines("- **  **", "  - nameless"),
  lines("# Heading template", "- List", "  - child"),
  lines("1. Numbered", "   - child", "2. Second", "   1. ordered child", "   2. another"),
  lines("- Paragraph body", "  - First", "", "    continued paragraph", "  - Last"),
  lines("- Code body", "  - ```js", "    const a = {cursor};", "    ```", "  - after"),
  lines("- Quote body", "  - > quoted {today}", "  - plain"),
  lines("- Unicode مرحبا 🙂", "  - {ask: کون؟} 🚀 {cursor} é", "    - {page}"),
  lines("---", "title: Templates", "---", "- Front", "  - body {page}"),
  lines("- Deep", "  - a", "    - b", "      - c", "        - d {cursor}", "  - e"),
  lines("- Empty child", "  - ", "  - x"),
  lines("- Task", "  - [ ] {ask: Waiting on whom?}: {cursor} #waiting 📅 {date: +3}"),
  lines("* Star", "  + plus child", "    * star grandchild"),
  lines("- Continuation", "  - Line one", "    line two {cursor}", "    line three ^cont", "  - ((cont))"),
  lines("- Embedded braces", "  - {{embed: [[Page]]}} {{pdf: file.pdf}} {cursor}", "  - {ask: a {b} c}"),
  "- Without newline\n  - last {cursor}",
  lines("- Child with id only ^root", "  - ^solo", "  - ((solo))"),
  lines("- Trailing spaces", "  - text   ", "  - more\t"),
  lines("- Glued", "  - {today}{today}{cursor}{time}"),
  lines("- Mixed case name", "  - x", "- mixed case name", "  - y"),
  starterTemplates,
  templateExamples.map(templateDefinition).join(""),
];
const fragments = ["- Name", "- Other name", "  - child", "  - [[{ask: Who?}]]", "  - {cursor}", "    - grandchild {time}", "  - {{tasks: next}}", "  - {date: +2}", "  - {pick: #meeting}", "  - item ^id1", "  - see ((id1))", "  - [[Page]] #tag", "  - {weekday} {page}", "Paragraph", "  - **bold** {today}", "  - ^id2", "    - ((id2)) {ask: Why?}"];
let seed = 0x7e3a11;
const random = (n: number) => { seed ^= seed << 13; seed ^= seed >>> 17; seed ^= seed << 5; return (seed >>> 0) % n; };
const seeded = Array.from({ length: 60 }, () => {
  const rows: string[] = [];
  for (let i = 0; i < 2 + random(9); i++) rows.push(i === 0 ? fragments[random(2)] : fragments[random(fragments.length)]);
  return rows.join("\n") + (random(3) ? "\n" : "");
});

const nows = [
  { date: new Date(2026, 9, 30, 9, 5), iso: "2026-10-30", hour: 9, minute: 5 },
  { date: new Date(2026, 11, 27, 23, 59), iso: "2026-12-27", hour: 23, minute: 59 },
  { date: new Date(2024, 1, 28, 0, 0), iso: "2024-02-28", hour: 0, minute: 0 },
];
const destinations = ["2026-10-27.md", "Projects/Plan.md"];
const layouts: DailyLayout[] = [
  { folder: "", title: "{YYYY}-{MM}-{DD}" },
  { folder: "Daily", title: "{MMMM} {DD}, {YYYY}" },
];

const attempt = async <T>(run: () => Promise<T> | T) => { try { return { ok: await run() }; } catch (error) { return { error: (error as Error).message }; } };

const pages: unknown[] = [];
for (const [index, source] of [...handwritten, ...seeded].entries()) {
  const text = editorText(source);
  const definitions = templatesInPage("Templates.md", text, parseBlocks(text));
  const expansions: unknown[] = [];
  const copies = new Map<string, unknown>();
  for (const definition of definitions) {
    counter = 0;
    const copied = copyWithNewIds(definition.body, destinations[0], ["Templates", "Templates"]);
    const copiedScoped = (counter = 0, copyWithNewIds(definition.body, destinations[1], ["Templates", "Templates"]));
    for (const [layoutIndex, layout] of layouts.entries()) {
      if (layoutIndex > 0 && index % 4) continue;
      setActiveDailyLayout(layout);
      for (const [nowIndex, now] of nows.entries()) {
        for (const destination of destinations) {
          if (destination !== destinations[0] && nowIndex > 0) continue;
          const notebook = notebookOf({ "Templates.md": text, "2026-10-27.md": "- Target ^target\n", "Projects/Plan.md": "- Plan\n" });
          const listed = notebook.templates().find(item => item.key === definition.key && item.path === "Templates.md")!;
          for (const dismiss of [false, true]) {
            if (dismiss && (nowIndex > 0 || layoutIndex > 0 || destination !== destinations[0])) continue;
            const asked: { kind: string; question: string }[] = [];
            counter = 0;
            const result = await attempt(() => expandTemplate(notebook, listed, destination, {
              now: now.date,
              prompt: question => { asked.push({ kind: "ask", question }); return dismiss ? null : `A(${question})`; },
              pick: (request: PickRequest) => { asked.push({ kind: "pick", question: request.tag }); return dismiss ? null : `P(${request.tag})`; },
            }));
            const expanded = "ok" in result ? result.ok : undefined;
            expansions.push({
              key: definition.key,
              layout: layoutIndex,
              now: { date: now.iso, hour: now.hour, minute: now.minute },
              destination,
              title: notebook.title(destination),
              dismiss,
              asked,
              result: expanded ? { text: expanded.text, cursor: expanded.cursor } : result,
              preserve: expanded ? { parsed: parsed(expanded.text), text: preserveTemplateRoot(expanded.text, "trigger") } : null,
            });
          }
        }
      }
    }
    setActiveDailyLayout();
    copies.set(definition.key, [
      { destination: destinations[0], parsed: parsed(copied), text: copied },
      { destination: destinations[1], parsed: parsed(copiedScoped), text: copiedScoped },
    ]);
  }
  pages.push({
    name: `page-${index}`,
    text,
    parsed: parsed(text),
    definitions: definitions.map(({ key, name, body }) => ({ key, name, body, bodyParsed: parsed(body), copies: copies.get(key) })),
    expansions,
  });
}

const otherPath = { text: "- Meeting\n  - Agenda\n", paths: ["templates.md", "TEMPLATES.md", "Templates.MD", "Other.md", "folder/Templates.md", "Templates.markdown"] };
const paths = otherPath.paths.map(path => ({ path, definitions: templatesInPage(path, otherPath.text, parseBlocks(otherPath.text)).length }));

const phrases = ["", " ", "today", "TODAY", " Tomorrow ", "yesterday", "+0", "-0", "+1", "-1", "+31", "-365", "+9999", "+10000", "1", "++1", "+ 1", "mon", "Monday", "monday", "tu", "tue", "tues", "wednesday", "thu", "fri", "sat", "sun", "sunday", "sundays", "2027-01-02", "2026-02-30", "2026-2-3", "20260101", "someday", "next week", "1 week", "٣"];
const dates = nows.flatMap(now => phrases.map(phrase => ({ today: now.iso, phrase, result: (() => { try { return { ok: templateDate(phrase, now.date) }; } catch (error) { return { error: (error as Error).message }; } })() })));

const tagged = {
  "Templates.md": "- Meeting\n  - [[{pick: #meeting}]] #meeting\n",
  "2026-10-20.md": "- [[Ops review]] 10:00 #meeting\n- [[Team Sync]] #meeting\n- `[[Code]]` [[Real]] #meeting\n",
  "2026-10-28.md": "- [[Team Sync]] #meeting\n- [[2026-10-28]] [[Design crit|crit]] #meeting\n- [[October 28th, 2026]] [[Roam linked]] #meeting\n",
  "2026-10-29.md": "- [[team sync]] #meeting\n- #meeting with nobody named\n- [[Page#Heading]] #Meeting\n- [[ Spaced |x]] #MEETING\n",
  "October 30th, 2026.md": "- [[Weekly]] #meeting\n",
  "Daily/October 31, 2026.md": "- [[Weekly]] #meeting\n- #meeting alone\n",
  "Board prep.md": "- #meeting\n- notes #meeting #meeting\n",
  "Projects/Alpha.md": "---\ntitle: Alpha project\n---\n- #meeting kickoff\n- [[2026-10-01]] #meeting\n",
  "Untagged.md": "- [[Not a meeting]]\n",
  "Other.md": "- [[Ops review]] #meeting/sub\n- ((block)) [[Zed]] #meeting\n- ![[Embedded]] #meeting\n",
};
const tagNotebook = notebookOf(tagged);
const past = ["#meeting", "meeting", "#Meeting", "#meeting/sub", "#none"].flatMap(tag => layouts.map((layout, layoutIndex) => {
  setActiveDailyLayout(layout);
  const uses = tagNotebook.taggedBlocks(tag.startsWith("#") ? tag : `#${tag}`).map(match => ({
    path: match.path,
    title: tagNotebook.title(match.path),
    links: references(match.snippet).filter(ref => ref.kind === "page").map(ref => ref.target),
  }));
  const names = pastNames(tagNotebook, tag.startsWith("#") ? tag : `#${tag}`);
  setActiveDailyLayout();
  return { tag, layout: layoutIndex, uses, names };
}));

const fuzzyNames = ["Tests", "Team Sync", "Ops review", "Ops", "Quarterly planning", "Design crit", "🚀 Launch review", "Ärger im Büro", "a-b_c/d.e:f(g[h", "camelCaseName", "ALLCAPS", "x", "", "İstanbul", "ß straße", "مرحبا بالعالم"];
const queries = ["", "  ", "t", "ts", "tsy", "t s", "ops", "OPS", "qp", "dcr", "zz", "rv", "ärg", "abc", "cn", "CCN", "acn", "allcaps", "x", "xx", "i̇", "ist", "ss", "ß", "مر", "e", "ew", "review", "a b c", "ab", "b_c", "plan"];
const fuzzy = {
  names: fuzzyNames,
  matches: queries.flatMap(query => fuzzyNames.map(name => ({ query, name, match: fuzzyMatch(query, name) ?? null }))),
  filters: queries.map(query => ({ query, order: fuzzyFilter(fuzzyNames, query, name => name).map(({ order }) => order) })),
};

const guide = {
  placeholders: templatePlaceholders,
  rules: templateRules,
  examples: templateExamples.map(example => ({ ...example, definition: templateDefinition(example) })),
  starters: starterTemplates,
};

writeFileSync("tests/fixtures/template-vectors.json", JSON.stringify({ pages, paths, dates, past, fuzzy, guide }) + "\n");
const expansionCount = pages.reduce((sum: number, page) => sum + (page as { expansions: unknown[] }).expansions.length, 0);
console.log(`Generated ${pages.length} Templates pages with ${expansionCount} expansions, ${dates.length} dates, ${past.length} past-name cases, and ${fuzzy.matches.length} fuzzy matches from the frozen template modules.`);
