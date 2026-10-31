// Generates tests/fixtures/relocate-vectors.json from the frozen rt planRelocation
// (tests/reference/core/relocate.ts). Each case gives a small notebook, a rename or move, and
// what the reference plans: every rewritten page's text and the listed link changes, or the
// refusal. The inputs Quire's analysis supplies natively ride along: code ranges, Markdown URL
// node ranges, and how each reference target resolves, so the Rust port is compared with the
// same parse. Run: bun tools/relocate-vectors.ts
import { writeFileSync } from "node:fs";
import { editorText, pageName, pageTitle, parseMarkdown, references } from "../tests/reference/markdown/markdown";
import { planRelocation, type Notebook } from "../tests/reference/core/relocate";

type Page = { path: string; text: string };

// The reference store's lookups over a fixed set of pages: an exact path name, then a unique
// title. Date links are left out; no generated page has a date title.
function notebook(pages: Page[]): Notebook {
  const texts = new Map(pages.map((page) => [page.path, editorText(page.text)]));
  const title = (path: string) => pageTitle(path, texts.get(path) ?? "");
  const byName = new Map<string, string>();
  for (const path of texts.keys()) byName.set(pageName(path).toLocaleLowerCase(), path);
  const pageKey = (target: string) => target.split("#")[0].replace(/\.md$/i, "").toLocaleLowerCase();
  return {
    documents: new Map([...texts.keys()].map((path) => [path, { revision: "r", state: { lineBreak: "\n" } }])),
    pagePaths: () => [...texts.keys()].sort((a, b) => title(a).localeCompare(title(b))),
    title,
    findPage(target: string) {
      const key = pageKey(target);
      const exact = byName.get(key);
      if (exact) return exact;
      const titled = [...texts.keys()].filter((path) => title(path).toLocaleLowerCase() === key);
      return titled.length === 1 ? titled[0] : undefined;
    },
    all: () => [...texts].map(([path, text]) => ({ path, text })),
  };
}

function analysis(text: string) {
  const tree = parseMarkdown(text);
  const codeRanges: [number, number][] = [];
  const urls: [number, number][] = [];
  tree.iterate({
    enter(node) {
      if (["FencedCode", "CodeBlock", "InlineCode"].includes(node.name)) codeRanges.push([node.from, node.to]);
      if (node.name === "URL") urls.push([node.from, node.to]);
    },
  });
  return { codeRanges, urls };
}

// Byte offsets for Rust: the vectors carry UTF-16 offsets from Lezer, converted here.
function bytes(text: string, ranges: [number, number][]) {
  const offset = (index: number) => Buffer.byteLength(text.slice(0, index));
  return ranges.map(([from, to]) => [offset(from), offset(to)]);
}

function vector(name: string, pages: Page[], source: string, target: string, rename: boolean) {
  const model = notebook(pages);
  const normalized = pages.map((page) => ({ path: page.path, text: editorText(page.text) }));
  const resolutions: Record<string, string | null> = {};
  for (const page of normalized)
    for (const reference of references(page.text)) {
      const hash = reference.target.indexOf("#");
      const base = hash < 0 ? reference.target : reference.target.slice(0, hash);
      resolutions[base] = model.findPage(base) ?? null;
    }
  const sourceTitle = model.title(source);
  const uniqueTitle =
    model.pagePaths().filter((path) => model.title(path).toLocaleLowerCase() === sourceTitle.toLocaleLowerCase()).length === 1;
  let result: unknown;
  try {
    const plan = planRelocation(model, source, target, { rename });
    result = {
      target: plan.target,
      texts: Object.fromEntries(plan.edits.filter((edit) => !edit.remove).map((edit) => [edit.path, edit.text])),
      updates: plan.updates,
    };
  } catch (error) {
    result = { error: String(error).replace(/^Error: /, "") };
  }
  return {
    name,
    source,
    target,
    rename,
    sourceTitle,
    uniqueTitle,
    resolutions,
    pages: normalized.map((page) => {
      const { codeRanges, urls } = analysis(page.text);
      return { ...page, codeRanges: bytes(page.text, codeRanges), urls: bytes(page.text, urls) };
    }),
    result,
  };
}

const cases: ReturnType<typeof vector>[] = [];

// The rt relocate.test.ts cases (`35e673677291`).
const moved = [
  { path: "Old.md", text: "- Text #tag ^stable\n\n![Image](assets/photo.png)\n\n![[assets/paper.pdf]]\n" },
  { path: "Links.md", text: "[[Old]] [[Old#^stable]] [Old](Old.md)\n`[code](Old.md)`" },
];
cases.push(vector("rt-move-with-links-and-attachments", moved, "Old.md", "Archive/New.md", false));
const collisions = [
  { path: "Old.md", text: "Original" },
  { path: "Exists.md", text: "Existing" },
];
cases.push(vector("rt-traversal", collisions, "Old.md", "../Escape", false));
cases.push(vector("rt-collision", collisions, "Old.md", "exists.md", false));
cases.push(vector("rt-case-only", collisions, "Old.md", "old.md", true));
cases.push(vector("rt-adds-md", collisions, "Old.md", "Archive/New", false));
const titled = [
  { path: "Old name.md", text: "- Body\n" },
  { path: "Titled.md", text: '---\ntags: [x]\ntitle: "Titled"\n---\n\n- title: in the body\n' },
  { path: "Links.md", text: "- [[Old name]] and [[Old name#Part|alias]] and [[Titled]]\n" },
];
cases.push(vector("rt-rename", titled, "Old name.md", "New name.md", true));
cases.push(vector("rt-rename-front-matter-title", titled, "Titled.md", "Renamed.md", true));
cases.push(vector("rt-move-keeps-title", titled, "Old name.md", "Archive/Old name 2.md", false));

// Every link form, from pages beside and away from the page, for renames and moves.
const forms = [
  "[[OLD]]", "[[OLD|alias]]", "[[OLD#Part]]", "[[OLD#Part|alias]]", "[[OLD#^stable]]", "[[old-lower]]",
  "![[OLD]]", "![[OLD#^stable]]", "{{embed: [[OLD]]}}", "{{[[embed]]: [[OLD]]}}", "{{embed: ((stable))}}",
  "((stable))", "[[PATHNAME]]", "[[PATHNAME.md]]", "[[TITLE]]", "[[Other]]", "[[Other#^stable]]",
  "[x](RELFILE)", "[x](RELFILE#part)", "[x](<RELSPACE>)", "[x](RELENCODED)", "[x](./RELFILE)",
  "[x](/ROOTFILE)", "![img](assets/photo.png)", "![[assets/paper.pdf]]", "{{pdf: assets/paper.pdf}}",
  "<https://x.test/Old.md>", "www.example.test/Old.md", "`[[OLD]]`", "[ref]: RELFILE", "[x](Other.md)",
  "[x](mailto:someone@example.test)", "[x](missing/../Other.md)",
];
const sources = [
  { path: "Old.md", title: undefined },
  { path: "Old name.md", title: undefined },
  { path: "Notes/Old.md", title: undefined },
  { path: "Notes/Old name.md", title: "Shown Title" },
];
const linkers = ["Links.md", "Notes/Links.md", "Deep/Sub/Links.md"];
const relative = (from: string, target: string) => {
  const a = from.split("/").slice(0, -1), b = target.split("/");
  while (a.length && b.length && a[0] === b[0]) { a.shift(); b.shift(); }
  return [...a.map(() => ".."), ...b].join("/");
};
function linkText(form: string, source: string, linker: string, title: string) {
  const name = pageName(source).split("/").at(-1)!;
  const rel = relative(linker, source);
  return form
    .replaceAll("OLD", name)
    .replace("old-lower", name.toLowerCase())
    .replace("PATHNAME", pageName(source))
    .replace("TITLE", title)
    .replace("RELSPACE", rel)
    .replace("RELENCODED", rel.replaceAll(" ", "%20"))
    .replace("RELFILE", rel.includes(" ") ? `<${rel}>` : rel)
    .replace("ROOTFILE", source);
}
const selfContent = [
  "- Body ^stable\n",
  "---\ntitle: \"Shown Title\"\n---\n\n- Body ^stable\n",
  "---\ntags: [x]\n---\n- Body ^stable [[Old]] ![[assets/paper.pdf]] ![i](../assets/photo.png)\n",
  "﻿---\ntitle: Shown Title\n---\n- Body ^stable [x](Other.md) [y](./Other.md#h)\n",
  "- Body ^stable\n  - [[Other]] <https://x.test> [z](sub/Child.md)\n",
];
let index = 0;
for (const source of sources)
  for (const linker of linkers) {
    if (linker === source.path) continue;
    const content = selfContent[index % selfContent.length];
    const own = source.title && !content.includes("title:") ? `---\ntitle: "${source.title}"\n---\n${content}` : content;
    const title = pageTitle(source.path, own);
    const links = forms.map((form) => `- ${linkText(form, source.path, linker, title)}`).join("\n") + "\n";
    const pages = [
      { path: source.path, text: own },
      { path: linker, text: links },
      { path: "Other.md", text: "- other ^other\n" },
    ];
    const folder = source.path.split("/").slice(0, -1).join("/");
    const sibling = (name: string) => (folder ? `${folder}/${name}` : name);
    cases.push(vector(`forms-${index}-rename`, pages, source.path, sibling("Renamed page.md"), true));
    cases.push(vector(`forms-${index}-move`, pages, source.path, `Archive/${source.path.split("/").at(-1)}`, false));
    cases.push(vector(`forms-${index}-rename-crlf`, pages.map((page) => ({ ...page, text: page.text.replaceAll("\n", "\r\n") })), source.path, sibling("Renamed page.md"), true));
    index++;
  }

// Refusals and edge names.
const plain = [{ path: "Old.md", text: "- old\n" }, { path: "Other.md", text: "- [[Old]]\n" }];
for (const [name, target] of [["taken", "Other.md"], ["taken-case", "OTHER.md"], ["slash-dot", "./x"], ["reserved", "con"], ["trailing-space", "Name .md"], ["unicode", "Café 🙂.md"], ["nested", "A/B/C.md"], ["empty-part", "A//B"]] as const)
  cases.push(vector(`target-${name}`, plain, "Old.md", target, true));
cases.push(vector("missing-source", plain, "Gone.md", "New.md", true));

// Duplicate titles: a move keeps links that name the title only when it is unique.
const duplicates = [
  { path: "A/Note.md", text: "- a\n" },
  { path: "B/Note.md", text: "- b\n" },
  { path: "Links.md", text: "- [[Note]] [[A/Note]] [[B/Note#^x]]\n" },
];
cases.push(vector("duplicate-title-move", duplicates, "A/Note.md", "C/Note.md", false));
cases.push(vector("duplicate-title-rename", duplicates, "A/Note.md", "A/Memo.md", true));

// Seeded mixtures of forms across several linkers.
let state = 0x5eed42;
const random = (n: number) => { state ^= state << 13; state ^= state >>> 17; state ^= state << 5; return (state >>> 0) % n; };
for (let seed = 0; seed < 120; seed++) {
  const source = sources[random(sources.length)];
  const own = selfContent[random(selfContent.length)];
  const pages = [{ path: source.path, text: own }];
  const title = pageTitle(source.path, own);
  for (const linker of linkers.filter((path) => path !== source.path).slice(0, 1 + random(2))) {
    const lines = Array.from({ length: 2 + random(6) }, () => {
      const parts = Array.from({ length: 1 + random(3) }, () => linkText(forms[random(forms.length)], source.path, linker, title));
      return (random(3) ? "- " : random(2) ? "  - " : "") + parts.join(random(2) ? " " : " and ");
    });
    pages.push({ path: linker, text: lines.join("\n") + (random(4) ? "\n" : "") });
  }
  pages.push({ path: "Other.md", text: "- other\n" });
  const folder = source.path.split("/").slice(0, -1).join("/");
  const rename = random(3) !== 0;
  const target = rename ? `${folder ? folder + "/" : ""}Fresh ${seed}.md` : `Moved/${seed}/${source.path.split("/").at(-1)}`;
  cases.push(vector(`seeded-${seed}`, pages, source.path, target, rename));
}

writeFileSync("tests/fixtures/relocate-vectors.json", "[\n" + cases.map((item) => JSON.stringify(item)).join(",\n") + "\n]\n");
console.log(`Generated ${cases.length} relocation vectors from the frozen planRelocation.`);
