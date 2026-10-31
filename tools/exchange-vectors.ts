// Generates tests/fixtures/exchange-vectors.json from the frozen rt Markdown export and import
// code: exportMarkdown (tests/reference/core/page-export.ts), exportName and localizeAttachments
// (core/export-files.ts), and the importMarkdown planning (core/markdown-import.ts). Each export
// case records what the reference notebook answered (references per text, block resolution,
// page lookup, titles), so the Rust port is compared with the same answers.
// Run: bun tools/exchange-vectors.ts
import { writeFileSync } from "node:fs";
import { pageName, pageTitle, parseBlocks } from "../tests/reference/markdown/markdown";
import { exportMarkdown } from "../tests/reference/core/page-export";
import { parses } from "../tests/reference/core/export-markdown";
import { exportName, localizeAttachments } from "../tests/reference/core/export-files";
import { importMarkdown, type Entry } from "../tests/reference/core/markdown-import";
import type { Notebook } from "../tests/reference/core/export-notebook";

type Docs = Record<string, string>;

// The reference store's lookups over fixed pages: an exact path name, then a unique title, and
// block identifiers across every page.
function notebook(docs: Docs, log: { resolves: Record<string, unknown>; finds: Record<string, string | null>; titles: Record<string, string> }): Notebook {
  const paths = Object.keys(docs);
  const title = (path: string) => pageTitle(path, docs[path] ?? "");
  const pageKey = (target: string) => target.split("#")[0].replace(/\.md$/i, "").toLocaleLowerCase();
  const identifiers = new Map<string, { path: string; block: ReturnType<typeof parseBlocks>[number] }[]>();
  for (const path of paths)
    for (const block of parseBlocks(docs[path]))
      if (block.id) identifiers.set(block.id, [...(identifiers.get(block.id) ?? []), { path, block }]);
  const findPage = (target: string) => {
    const key = pageKey(target);
    const exact = paths.find((path) => pageName(path).toLocaleLowerCase() === key);
    if (exact) return exact;
    const titled = paths.filter((path) => title(path).toLocaleLowerCase() === key);
    return titled.length === 1 ? titled[0] : undefined;
  };
  return {
    text: (path) => docs[path] ?? "",
    title: (path) => (log.titles[path] = title(path)),
    findPage: (target) => {
      const found = findPage(target);
      log.finds[target] = found ?? null;
      return found;
    },
    resolve: (target) => {
      const separator = target.indexOf("#^");
      const id = separator >= 0 ? target.slice(separator + 2) : target;
      const page = separator >= 0 ? findPage(target.slice(0, separator)) : undefined;
      const matches = (identifiers.get(id) ?? []).filter((item) => separator < 0 || item.path === page);
      const found = matches.length === 1 ? matches[0] : undefined;
      log.resolves[target] = found
        ? { path: found.path, from: found.block.from, contentFrom: found.block.contentFrom, subtreeTo: found.block.subtreeTo, text: found.block.text }
        : null;
      return found ?? { error: "missing or ambiguous" };
    },
  };
}

const lines = (...rows: string[]) => rows.join("\n") + "\n";
const cases: { name: string; docs: Docs; path: string }[] = [
  {
    name: "rt page-export.test.ts: references resolved",
    path: "Plan.md",
    docs: {
      "Plan.md": "# Plan\n\n- Goals for [[Launch|the launch]] ➕ [[2026-10-29]] ^goal\n  - Ship by **December**\n- See ((goal))\n- {{embed: ((src))}}\n- ![[diagram.png]]\n- ![[Notes]]\n- ![Chart](img/chart.png)\n- [[Missing page]] stays as text\n",
      "Source.md": "- Embedded root ^src\n  - Embedded child [[Plan]]\n",
      "Notes.md": "- Note body\n",
    },
  },
  {
    name: "cycles stop at the nesting limit",
    path: "A.md",
    docs: { "A.md": "- A embeds\n  - ![[B]]\n", "B.md": "- B embeds\n  - ![[A]] ^b\n", "C.md": "- ((b)) and ((c1)) ^c1\n" },
  },
  {
    name: "block embeds re-indent children under the embed column",
    path: "Host.md",
    docs: {
      "Host.md": lines("- Top", "    - {{embed: ((deep))}}", "1. ![[Lib#^deep]]", "Paragraph ![[Lib#^deep]] inline", "\t- \t![[Lib]]"),
      "Lib.md": lines("- Library", "  - Deep root ^deep", "    - Child one ➕ 2026-01-02", "", "      - Grandchild ^gc", "  - Sibling"),
    },
  },
  {
    name: "media, files, schemes and escapes",
    path: "Folder/Media.md",
    docs: {
      "Folder/Media.md": lines(
        "- {{pdf: papers/Report.pdf}} and {{[[pdf]]: ../top.pdf#page=2}}",
        "- ![[report.docx]] ![[sheet.CSV]] ![[Notes.md]] ![[x.MD]]",
        "- ![a](../up.png) ![b](https://example.com/b.png) ![c](//cdn/c.png) ![d](data:image/png;base64,AA)",
        "- ![e](<spaced name.png>) ![f](café%20au%20lait.png \"Title\") ![g](../../outside.png) ![h](/root.png)",
        "- ![[photo.jpeg|Holiday]] ![[scan.pdf?x=1]] ![[img%20one.webp]] ![[../escape.png]]",
        "- `![[Notes]]` and ```((goal))``` stay as code",
      ),
      "Notes.md": "- Note\n",
    },
  },
  {
    name: "malformed image escape",
    path: "Bad.md",
    docs: { "Bad.md": "- ![x](100%.png)\n" },
  },
  {
    name: "labels, duplicates, Unicode and missing blocks",
    path: "مرحبا.md",
    docs: {
      "مرحبا.md": lines(
        "- عربي [[Seite|Label]] ((dup)) ((none)) ((uni))",
        "  - * nested ![[Seite]]",
        "  - + plus {{[[embed]]: [[Seite]]}}",
        "  - 12) numbered ((uni|ignored))",
      ),
      "Seite.md": lines("---", "title: Die Seite", "---", "- Körper ^dup", "  - Kind ^uni", "- Zwei ^dup"),
      "Other.md": "- Ünïcödé text ➕ [[2026-10-29]] ^uni2\n",
    },
  },
  {
    name: "titles and front matter on embedded pages",
    path: "Index.md",
    docs: {
      "Index.md": lines("- ![[Die Seite]]", "- ![[Topic]]", "", "![[Topic]]", "> quote ![[Topic]]"),
      "Topic.md": lines("---", "title: Topic Title", "tags: [x]", "---", "", "Body line ^t1", "", "- item"),
      "Seite.md": lines("---", "title: Die Seite", "---", "- Körper"),
    },
  },
];

// Seeded pages mixing the fragments above, so combinations are exercised too.
let seed = 7;
const random = () => ((seed = (seed * 1103515245 + 12345) % 2147483648) / 2147483648);
const pick = <T,>(items: T[]) => items[Math.floor(random() * items.length)];
const fragments = [
  "[[P1]]", "[[P2|two]]", "((b1))", "((b2))", "![[P1]]", "![[P2#^b2]]", "{{embed: ((b1))}}", "![[pic.png]]",
  "![x](img/x.png)", "➕ 2026-05-05", "**bold**", "`code ((b1))`", "#tag", "![[file.zip]]", "{{pdf: d.pdf}}", "plain",
];
for (let index = 0; index < 40; index++) {
  const page = (name: string, id: string) =>
    Array.from({ length: 2 + Math.floor(random() * 4) }, (_, line) => {
      const indent = "  ".repeat(Math.floor(random() * 3));
      const marker = pick(["- ", "- ", "1. ", "* ", ""]);
      const words = Array.from({ length: 1 + Math.floor(random() * 3) }, () => pick(fragments)).join(" ");
      return `${line ? indent : ""}${marker}${name} ${words}${line === 1 ? ` ^${id}` : ""}`;
    }).join("\n") + "\n";
  cases.push({
    name: `seeded ${index}`,
    path: "Root.md",
    docs: { "Root.md": page("root", "r"), "P1.md": page("one", "b1"), "P2.md": page("two", "b2") },
  });
}

const exports = cases.map(({ name, docs, path }) => {
  parses.clear();
  const log = { resolves: {}, finds: {}, titles: {} };
  let output: string | null = null;
  let error = false;
  try {
    output = exportMarkdown(notebook(docs, log), path);
  } catch {
    error = true;
  }
  const references = [...parses].map(([text, found]) => ({
    text,
    references: found.map(({ kind, syntax, target, from, to, label, media }) => ({ kind, syntax, target, from, to, label: label ?? null, media: media ?? null })),
  }));
  return { name, docs, path, output, error, references, ...log };
});

const localTexts = [
  "![a](quire-vault:img%2Fchart.png) ![b](quire-vault:..%2Fsecret.txt) ![c](quire-vault:img%2Fescape.png) ![d](quire-vault:img%2Fgone.png)",
  "[x](quire-vault:a%2Fb%20c.pdf) again ![y](quire-vault:a%2Fb%20c.pdf) bad ![z](quire-vault:%E0%A4%A) ok",
  "no links here ](quire-vault:) ](quire-vault:x y)",
];
const available: Record<string, string> = { "img/chart.png": "/notes/img/chart.png", "a/b c.pdf": "/notes/a/b c.pdf" };
const localized = localTexts.map((text) => {
  const asked: string[] = [];
  const output = localizeAttachments(text, (relative) => {
    asked.push(relative);
    return available[relative];
  });
  return { text, output, asked };
});
const names = ['Q3: plan / "draft"', "  ", "", "Tabs\tand\nlines", "  padded  ", "a<b>c|d?e*f\\g", "﻿bom　"].map((title) => ({
  title,
  name: exportName(title, "md"),
}));

const file = (name: string): Entry => ({ name, file: true });
const folder = (name: string, children: Entry[]): Entry => ({ name, children });
const imports = [
  {
    name: "rt engine.test.ts: files and an existing page",
    source: [file("Plan.md"), folder("People", [file("Maram.md")]), folder(".obsidian", [file("x.md")]), folder("assets", [file("a.png")])],
    existing: ["Plan.md"],
  },
  {
    name: "case, Unicode normalization and duplicate keys",
    source: [file("plan.MD"), file("Café.md"), file("Café.md"), folder("Deep", [folder("Er", [file("Note.md"), file(".hidden.md"), file("link")]), file("NOTE.md")]), { name: "socket" }],
    existing: ["PLAN.md", "deep/er/note.md"],
  },
  {
    name: "an unusable path refuses",
    source: [file("Good.md"), file("CON.md")],
    existing: [],
  },
  {
    name: "names with spaces, dots and other files",
    source: [file("a b.md"), file("notes.txt"), folder("x", [file("y.markdown"), file("z.Md")]), file(".DS_Store")],
    existing: [],
  },
].map(({ name, source, existing }) => {
  try {
    return { name, source, existing, result: importMarkdown(source, existing), error: false };
  } catch {
    return { name, source, existing, result: null, error: true };
  }
});

writeFileSync("tests/fixtures/exchange-vectors.json", JSON.stringify({ exports, localized, names, imports }) + "\n");
console.log(`Generated ${exports.length} export cases, ${localized.length} attachment rewrites, ${names.length} names, and ${imports.length} import plans from the frozen rt exchange modules.`);
