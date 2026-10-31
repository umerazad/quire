// Generates tests/fixtures/roam-vectors.json from the frozen rt Roam exchange code: importRoamJSON
// and exportRoamJSON (tests/reference/core/roam.ts), importRoamEDN (core/roam-edn.ts) and the
// EDN reader (core/edn.ts). Each case records the reference parses roam.ts asked for (references
// and blocks per text) and uses counted identifiers, so the Rust port answers the same texts with
// the same parses and generates the same identifiers.
// Run: bun tools/roam-vectors.ts
import { writeFileSync } from "node:fs";
import { importRoamJSON, exportRoamJSON, type RoamMetadata } from "../tests/reference/core/roam";
import { importRoamEDN } from "../tests/reference/core/roam-edn";
import { readEDN, ednJSON } from "../tests/reference/core/edn";
import { blockText } from "../tests/reference/markdown/markdown";
import { blockParses, parses, resetRecording } from "../tests/reference/core/roam-markdown";

type Docs = { path: string; text: string }[];

const message = (error: unknown) => (error instanceof Error ? error.message : String(error));

function recorded() {
  return {
    references: [...parses].map(([text, found]) => ({
      text,
      references: found.map(({ kind, syntax, target, from, to, label, media }) => ({
        kind,
        syntax,
        target,
        from,
        to,
        label: label ?? null,
        media: media ?? null,
      })),
    })),
    blocks: [...blockParses].map(([text, found]) => ({
      text,
      blocks: found.map((block) => ({
        key: block.key,
        id: block.id ?? null,
        parent: block.parent ?? null,
        from: block.from,
        subtreeTo: block.subtreeTo,
        text: blockText(text, block),
      })),
    })),
  };
}

function imported(result: ReturnType<typeof importRoamJSON>) {
  return { documents: result.documents, report: result.report, metadata: JSON.stringify(result.metadata) };
}

const jsonImports: { name: string; input: string }[] = [];
const ednImports: { name: string; input: string }[] = [];
const exportsIn: { name: string; documents: Docs; metadata: string | null; firstId: number }[] = [];

const json = (name: string, value: unknown) => jsonImports.push({ name, input: JSON.stringify(value) });
const raw = (name: string, input: string) => jsonImports.push({ name, input });

// rt tests/import.test.ts inputs.
json("rt: case-sensitive titles", [
  { title: "Case", children: [{ uid: "upper", string: "[[case]]" }] },
  { title: "case", children: [{ uid: "lower", string: "[[Case]]" }] },
]);
json(
  "rt: truncated and reserved titles",
  ["a".repeat(129) + "." + "b".repeat(52), "a".repeat(129) + " " + "b".repeat(52), "語".repeat(140), "CON", "..", "a".repeat(129)].map((title) => ({ title })),
);
json("rt: hierarchy and embeds", [
  { title: "One", children: [{ uid: "parent", string: "Parent", children: [{ uid: "child", string: "Child" }] }] },
  { title: "Two", children: [{ uid: "ref", string: "((child))" }, { uid: "embed", string: "{{embed: ((parent))}}" }] },
]);
json("rt: portable names and unresolved references", [
  { title: "A/B", children: [{ uid: "a", string: "((missing))" }] },
  { title: "a:b", children: [] },
]);
json("rt: hidden, trimmed and normalized names", [{ title: " A" }, { title: "A" }, { title: ".hidden" }, { title: "e\u0301" }, { title: "é" }]);
json("rt: not an array", {});
json("rt: duplicate block IDs", [{ title: "Page", children: [{ uid: "same", string: "A" }, { uid: "same", string: "B" }] }]);
const nested = [
  {
    title: "A/B",
    uid: "page-one",
    "create-time": 123,
    "edit-user": { ":user/uid": "writer" },
    children: [
      {
        uid: "parent",
        string: "Parent",
        heading: 2,
        open: false,
        "text-align": "center",
        children: [
          { uid: "child", string: "First line\nSecond line\n", "edit-time": 456 },
          { uid: "code", string: "```js\nconst example = '((child))';\n```" },
        ],
      },
      { uid: "task", string: "{{[[TODO]]}} Finish ((child))" },
      { uid: "embed", string: "{{[[embed]]: ((parent))}}" },
    ],
  },
];
json("rt: nested metadata, code, tasks and embeds", nested);
json("rt: edited block keeps metadata", [{ title: "Page", children: [{ uid: "one", string: "Original", "create-time": 1, "edit-time": 2, open: false }] }]);
json("rt: multiline list text", [
  { title: "Page", children: [{ uid: "literal", string: "- First\n2. Second", children: [{ uid: "actual-child", string: "Real child" }] }, { uid: "edited", string: "Before" }] },
]);
json("rt: empty block with descendants", [{ title: "Page", children: [{ uid: "empty", string: "", children: [{ uid: "child", string: "Child" }] }] }]);
json("rt: unsupported IDs", [{ title: "Page", children: [{ uid: "space id", string: "Original" }, { uid: "ref", string: "((space id)) and `((space id))`" }] }]);
json("rt: null page", [null]);
json("rt: children not an array", [{ title: "Page", children: {} }]);

// Edge cases around the same rules.
json("page uid errors", [{ title: "P", uid: "" }]);
json("numeric uid", [{ title: "P", children: [{ uid: 5, string: "x" }] }]);
json("null uid", [{ title: "P", children: [{ uid: null, string: "x" }] }]);
json("missing string", [{ title: "P", children: [{ uid: "a" }] }]);
json("blank title", [{ title: "   " }]);
json("repeated title", [{ title: "Same" }, { title: "Same" }]);
json("page uid shared with a block", [{ title: "P", uid: "x", children: [{ uid: "x", string: "y" }] }]);
const chain = (levels: number) => {
  let node: Record<string, unknown> = { uid: "leaf", string: "leaf" };
  for (let depth = 0; depth < levels; depth++) node = { uid: `n${depth}`, string: `level ${depth}`, children: [node] };
  return [node];
};
json("too deep", [{ title: "Deep", children: chain(129) }]);
json("deep but allowed", [{ title: "Deep", children: chain(128) }]);
json("macros, images, tasks and headings", [
  {
    title: "Macros",
    children: [
      { uid: "m1", string: "{{query: {and: [[a]]}}}" },
      { uid: "m2", string: "{{embed: ((m1))}} {{pdf: x.pdf}} {{[[pdf]]: y.pdf}} {{TODO}} {{[[DONE]]}}" },
      { uid: "m3", string: "![a](https://example.com/a.png) ![b](http://x/b.png) ![c](local.png)" },
      { uid: "m4", string: "{{TODO}} first" },
      { uid: "m5", string: "{{[[DONE]]}} second", heading: 1 },
      { uid: "m6", string: "third", heading: 3 },
      { uid: "m7", string: "not heading", heading: 4 },
      { uid: "m8", string: "string heading", heading: "2" },
      { uid: "m9", string: "float heading", heading: 2.5 },
      { string: "no uid ((m1)) ((m9))" },
      { uid: "m10", string: "{{ roam/render: ((x)) }}" },
    ],
  },
]);
json("list markers and fences", [
  {
    title: "Lists",
    children: [
      { uid: "l1", string: "- dash\n+ plus\n* star\n1. one\n22) two\n   - indented three\n    - indented four\n-nospace" },
      { uid: "l2", string: "```\n- inside fence\n```\n- after fence" },
      { uid: "l3", string: "~~~~\n- tilde\n~~~ closes? no\n~~~~~ \n- after" },
      { uid: "l4", string: "``` lang\n- in\n````\n- out" },
      { uid: "l5", string: "line\r\nwith crlf\r\n- dash" },
      { uid: "l6", string: "```\r\n- cr fence\r\n```\r\n- cr after" },
      { uid: "l7", string: "trailing\n" },
      { uid: "l8", string: "\n" },
    ],
  },
]);
json("titles needing changes", [
  { title: "Q3: plan / \"draft\"" },
  { title: "tab\there" },
  { title: "trailing dots..." },
  { title: "Page CON" },
  { title: "con" },
  { title: "nul.txt" },
  { title: "<>" },
  { title: "x".repeat(250) },
  { title: "😀".repeat(70) },
  { title: "Ünïcödé" },
  { title: "Ünïcödé".normalize("NFD") },
  { title: "Untitled" },
  { title: "." },
  { title: "Daily/October 27th, 2026" },
  { title: "October 27th, 2026" },
]);
json("attributes keep their order", [
  { "create-time": 1, title: "Ordered", "10": "ten", "2": "two", uid: "pg", children: [{ "edit-time": 3, string: "s", uid: "b1", "1": "one", props: { b: 1, a: [1, 2, { c: null }] } }] },
]);
json("JSON number forms", [{ title: "Numbers", "create-time": 1.5e3, n: -0, big: 12345678901234567890, small: 1e-7, children: [{ uid: "a", string: "x", order: 0.1 }] }]);
json("replacement patterns and repeated texts", [
  {
    title: "Patterns",
    children: [
      { uid: "x", string: "((a$$b)) ((c$&d)) ((e$`f)) ((g$'h)) ((space id)) ((i$1j))" },
      { uid: "space id", string: "target" },
      { uid: "y", string: "((space id)) and ((space id))" },
      { uid: "z", string: "((space id)) and ((space id))" },
    ],
  },
]);
raw("duplicate keys keep the first position", '[{"title":"A","uid":"u1","title":"B","children":[]}]');
raw("escapes in strings", '[{"title":"Esc\\u00e9 \\"q\\" \\\\ \\/","children":[{"uid":"e","string":"tab\\tnew\\nline \\u2028 \\u0001 \\ud83d\\ude00"}]}]');
raw("whitespace around values", ' \n\t[ { "title" : "W" , "children" : [ ] } ] \r\n');

// Seeded graphs mixing the fragments above.
let seed = 11;
const random = () => (seed = (seed * 1103515245 + 12345) % 2147483648) / 2147483648;
const pick = <T,>(items: T[]) => items[Math.floor(random() * items.length)];
const fragments = [
  "plain", "[[P0]]", "[[Missing]]", "((u1))", "((u2))", "((nope))", "{{embed: ((u3))}}", "{{[[embed]]: [[P1]]}}", "{{[[TODO]]}}", "{{DONE}}",
  "`((u1))`", "**bold**", "#tag", "![](https://h/x.png)", "![](img/a.png)", "{{query: x}}", "1. item", "- dash", "\n```\n- code ((u2))\n```", "\nsecond line", "[label]([[P2]])", "![[P0]]", "[[P0#^u1]]",
];
const titles = ["P0", "P1", "P2", "p0 copy", "A/B", "Ünï", "Daily/Day", "Big " + "x".repeat(140), "con", " spaced "];
const seededGraphs: unknown[][] = [];
for (let index = 0; index < 30; index++) {
  let uid = 0;
  const block = (depth: number): Record<string, unknown> => {
    const id = ++uid;
    const node: Record<string, unknown> = {
      string: Array.from({ length: 1 + Math.floor(random() * 3) }, () => pick(fragments)).join(" "),
      uid: random() < 0.1 ? `bad id ${id}` : `u${id}`,
    };
    if (random() < 0.3) node["create-time"] = 1700000000000 + id;
    if (random() < 0.3) node["edit-time"] = 1700000000500 + id;
    if (random() < 0.2) node.heading = 1 + Math.floor(random() * 4);
    if (random() < 0.15) node.open = false;
    if (random() < 0.1) node["edit-user"] = { ":user/uid": "user" };
    if (depth < 3 && random() < 0.4) node.children = Array.from({ length: 1 + Math.floor(random() * 3) }, () => block(depth + 1));
    return node;
  };
  const used = new Set<string>();
  const pages = Array.from({ length: 2 + Math.floor(random() * 4) }, (_, page) => {
    let title = pick(titles);
    while (used.has(title)) title += " again";
    used.add(title);
    const result: Record<string, unknown> = { title };
    if (random() < 0.7) result.uid = `page-${index}-${page}`;
    if (random() < 0.5) result["create-time"] = 1600000000000 + page;
    if (random() < 0.9) result.children = Array.from({ length: Math.floor(random() * 4) }, () => block(0));
    return result;
  });
  seededGraphs.push(pages);
  json(`seeded ${index}`, pages);
}

// EDN databases: rt tests/roam-edn.test.ts and roam-files.test.ts inputs, then edge cases.
const database = (datoms: string, schema = "{:block/children {:db/cardinality :db.cardinality/many} :block/refs {:db/cardinality :db.cardinality/many}}") =>
  `#datascript/DB {\n  :schema ${schema}\n  :datoms [${datoms}]}`;
const page = '[1 :node/title "Page" 100] [1 :block/uid "page" 100]';
const block = (id: number, uid: string, text: string) => `[${id} :block/uid "${uid}" 100] [${id} :block/string ${JSON.stringify(text)} 100]`;
const edn = (name: string, input: string) => ednImports.push({ name, input });
edn("rt: order, hierarchy, references and users", database(`${page}
    [1 :block/children 2 100] [1 :block/children 3 100]
    ${block(2, "last", "((child))")} [2 :block/order 1 100]
    ${block(3, "first", "Parent")} [3 :block/order 0 100]
    [3 :block/children 4 100] [3 :block/open false 100] [3 :block/heading 2 100]
    ${block(4, "child", "Line\nλ")} [4 :block/order 0 100]
    [4 :create/time 123 100] [4 :create/user 5 100]
    [5 :user/uid "writer" 100] [5 :user/settings {:x #{1 2 3} :n ##NaN} 100]`));
edn("rt: orphan trees", database(`${page}
    ${block(2, "orphan", "Orphan")} [2 :block/children 3 100]
    ${block(3, "descendant", "Descendant")}
    [10 :version/id "version" 100] [10 :block/children 4 100]
    ${block(4, "history", "Detached history")}`));
edn("rt: empty database UID", database(`${page} ${block(2, "", "Recovered text")}`));
for (const [index, datoms] of [
  `${page} [1 :block/children 2 100]`,
  `${page} ${block(2, "a", "A")} [2 :block/children 2 100]`,
  `${page} ${block(2, "a", "A")} [1 :block/children 2 100] [3 :block/children 2 100]`,
  `${page} [1 :node/title "Duplicate" 100]`,
  `${page} [2 :block/string "A" 100 false]`,
  '[1 :node/title "Page"]',
].entries())
  edn(`rt: invalid ${index}`, database(datoms));
edn("rt: not a database", "[]");
edn("rt files: reserved title and settings", '#datascript/DB {:schema {} :datoms [[1 :node/title "CON" 10] [2 :user/settings {:x ##NaN :y #{1 2 3}} 10]]}');
edn("rt worker: one page", '#datascript/DB {:schema {} :datoms [[1 :node/title "Page" 10]]}');
edn("rt worker: not an export", "not an export");
edn("missing orders and ties", database(`${page}
    [1 :block/children 4 100] [1 :block/children 2 100] [1 :block/children 3 100] [1 :block/children 5 100]
    ${block(2, "b", "B")} ${block(3, "c", "C")} [3 :block/order 2 100]
    ${block(4, "d", "D")} [4 :block/order 2 100] ${block(5, "e", "E")} [5 :block/order 1.5 100]`));
edn("recovered title collision", database(`${page} [6 :node/title "Roam recovered blocks" 100] [6 :block/uid "r1" 100] [7 :node/title "Roam recovered blocks 2" 100] ${block(2, "o", "Orphan")}`));
edn("attributes and aliases", database(`${page} [1 :block/children 2 100] [1 :create/time 5 100] [1 :edit/user 9 100]
    ${block(2, "x", "Text ((y))")} [2 :block/text-align "center" 100] [2 :children/view-type :bullet 100] [2 :block/view-type :numbered 100]
    [2 :edit/time 77 100] [2 :block/refs 1 100] [2 :block/refs 3 100] [2 :block/props {:a "b" "c" 1 [1] 2} 100] [2 :entity/attrs #{[1 2]} 100]
    [2 :block/big 9007199254740993 100] [2 :block/tag #inst "2020-01-01" 100] [2 :block/sym some-symbol 100] [2 :block/char \\a 100]
    [9 :user/display-name "nobody" 100] [2 :create/user 9 100]`));
edn("single-valued attribute repeated", database(`${page} [1 :block/uid "again" 100]`));
edn("children cardinality one", database(`${page} ${block(2, "a", "A")} [1 :block/children 2 100]`, "{}"));
edn("child without text", database(`${page} [1 :block/children 2 100] [2 :block/uid "a" 100]`));
edn("page with text", database(`${page} [1 :block/string "x" 100]`));
edn("datom shapes", database(`${page} [1 :block/order]`));
edn("non-keyword attribute", database(`${page} [1 "title" "x" 100]`));
edn("string attribute with colon", database(`${page} [1 ":block/open" true 100]`));
edn("bad order", database(`${page} [1 :block/children 2 100] ${block(2, "a", "A")} [2 :block/order "1" 100]`));
edn("unsafe id", database(`[9007199254740993 :node/title "Big" 100]`));
edn("comments, discards and commas", `; header\n#datascript/DB {:schema {}, :datoms [#_ [9 :x 1 1] [1 :node/title "P" 1],]} ; trailing`);
edn("tagged root other", '#other/DB {:schema {} :datoms []}');
edn("empty datoms", '#datascript/DB {:schema {} :datoms []}');
edn("escapes and chars", database(`${page} [1 :block/children 2 100] [2 :block/uid "esc" 100] [2 :block/string "q\\"\\\\\\t\\u00e9\\n\\b\\f\\r" 100]`));
edn("deep nesting", database(`${page} [1 :block/children 2 100] ${Array.from({ length: 140 }, (_, i) => `${block(i + 2, `d${i}`, `D${i}`)} [${i + 2} :block/children ${i + 3} 100]`).join(" ")} ${block(142, "end", "End")}`));
edn("unicode outside the basic plane", database(`[1 :node/title "😀 page" 100] [1 :block/children 2 100] ${block(2, "u", "smile 😀 and λ")}`));

// Seeded EDN graphs: pages, ordered and unordered children, detached blocks, users.
for (let index = 0; index < 20; index++) {
  const datoms: string[] = [];
  let entity = 1;
  const pages = 1 + Math.floor(random() * 3);
  const blockIds: number[] = [];
  for (let p = 0; p < pages; p++) {
    const id = entity++;
    datoms.push(`[${id} :node/title ${JSON.stringify(`Page ${index}-${p}`)} 1]`, `[${id} :block/uid "p${index}${p}" 1]`);
    const add = (parent: number, depth: number) => {
      const count = Math.floor(random() * 3);
      for (let c = 0; c < count; c++) {
        const child = entity++;
        blockIds.push(child);
        datoms.push(`[${parent} :block/children ${child} 1]`, `[${child} :block/string ${JSON.stringify(pick(fragments))} 1]`);
        if (random() < 0.9) datoms.push(`[${child} :block/uid ${JSON.stringify(random() < 0.05 ? "" : `u${child}`)} 1]`);
        if (random() < 0.8) datoms.push(`[${child} :block/order ${Math.floor(random() * 4)} 1]`);
        if (random() < 0.2) datoms.push(`[${child} :create/time ${1700000000000 + child} 1]`);
        if (depth < 2) add(child, depth + 1);
      }
    };
    add(id, 0);
  }
  if (random() < 0.5) {
    const orphan = entity++;
    datoms.push(`[${orphan} :block/string "orphan ${index}" 1]`, `[${orphan} :block/uid "o${index}" 1]`);
  }
  for (let i = datoms.length - 1; i > 0; i--) {
    const j = Math.floor(random() * (i + 1));
    [datoms[i], datoms[j]] = [datoms[j], datoms[i]];
  }
  edn(`seeded ${index}`, database(datoms.join(" ")));
}

// EDN reader: rt's own cases and more forms.
const reads = [
  String.raw`{:text "Line\n\u03bb\t\"\\"
    :set #{1 2 3} ; comment
    :list (true false nil) :discard [#_ 99 1]
    :tag #uuid "test" :infinite [##NaN ##Inf ##-Inf]
    :char [\newline \) \u03bb]
    :large 9007199254740993N :decimal 1.5M
    :complex {[1 2] :value}}`,
  "{:a}", "[1)", '"unterminated', '"\\q"', "#=(do anything)", "{} {}", "#unknown", "[1", "{:a 1 :a 2}",
  "#_ 1 2", "[+5 -3 007 1e5 1E-2 2.5e+3 1n 1m 9007199254740992 -9007199254740991 1e400 0.1 -0]",
  "[\\space \\tab \\return \\backspace \\formfeed \\u0041 \\x \\abc]", "[a-symbol ns/sym + - ... :kw :ns/kw]", "{\"a\" 1 \"1\" 2 \"0\" 3}",
  "{:a 1 \":a\" 2}", "{1 :x 1.0 :y}", "{nil 1 false 2}", "#{}", "()", "#_ #_ 1 2 3", "# x", "#1abc 2", "[1 #_]", "##Other", "\\",
  "  [1 ,2]﻿", "\"\\u00e9\\uD83D\\uDE00\"", "\"bad \\u12\"", "[1 2", "[#inst \"x\" #_ 1]", "#a/b #c/d 1",
  "[:a;comment\n:b]", "{:k #{1 #{2}} :m {:n {:o [()]}}}", "123abc", "#abc", "[1.5.5]", ":", "[true false nil truex]",
  "[" + "[".repeat(256) + "]".repeat(256) + "]", "[" + "[".repeat(255) + "]".repeat(255) + "]",
];

// Markdown notebooks exported without Roam metadata.
// Round trips number export identifiers from 1001, as random identifiers never meet the import's.
const exportCase = (name: string, documents: Docs, metadata?: unknown, firstId = 1) =>
  exportsIn.push({ name, documents, metadata: metadata === undefined ? null : JSON.stringify(metadata), firstId });
exportCase("rt: native hierarchy and qualified references", [
  { path: "Folder/A.md", text: "- Parent ^parent\n  - Child ^child\n    - [ ] Task ^task\n- ## Heading ^heading\n" },
  { path: "B.md", text: "![[Folder/A#^parent]]\n\n[[Folder/A#^child]]\n\n![image](assets/photo.png)\n\n![[Folder/A]]\n" },
]);
exportCase("rt: duplicate block IDs", [{ path: "A.md", text: "- A ^same" }, { path: "B.md", text: "- B ^same" }]);
exportCase("rt: duplicate titles", [{ path: "A.md", text: '---\ntitle: "Same"\n---\nA' }, { path: "B.md", text: '---\ntitle: "Same"\n---\nB' }]);
exportCase("front matter, gaps, CRLF and links", [
  { path: "Notes/Plan.md", text: "---\ntitle: Launch plan\ntags: [a]\nowner: me\n---\nIntro paragraph [[Other]]\n\n- [x] Done thing\n- [X] Also\n- [ ]\n- # One\n  - ### Three\n- #### Four\n\n> quote ((b1))\n\n```\ncode [[Other]]\n```\n" },
  { path: "Other.md", text: "Line one\r\n- item ^b1\r\n  - child [[notes/plan|label]] ![[missing.pdf]] ![[Nope]] {{pdf: x.pdf}} ![[Other#^b1]] [[Plan#^b1]]\r\n" },
  { path: "Third.md", text: "---\ntitle: Third\n---\n" },
  { path: "Empty.md", text: "" },
  { path: "Media.md", text: "- ![[photo.JPG]] ![[clip.mp4]] [doc](files/doc.pdf) [web](https://x.y) [mail](mailto:a@b) [frag](#x) ![d](data:image/png;base64,AA)\n" },
]);
exportCase("metadata file shapes", [{ path: "A.md", text: "- a\n" }], { version: 2, pages: {} });
exportCase("metadata pages not an object", [{ path: "A.md", text: "- a\n" }], { version: 1, pages: [] });
exportCase("metadata page without title", [{ path: "A.md", text: "- a\n" }], { version: 1, pages: { "A.md": { children: [] } } });
exportCase("metadata page with bad children", [{ path: "A.md", text: "- a\n" }], { version: 1, pages: { "A.md": { title: "A", children: [{ uid: "x" }] } } });
exportCase("metadata found by title", [{ path: "Renamed.md", text: "---\ntitle: \"Original\"\n---\n\n- kept ^k1\n" }], {
  version: 1,
  pages: { "Original.md": { title: "Original", uid: "pg", "create-time": 9, children: [{ uid: "k1", string: "kept", "edit-time": 4 }] } },
});
exportCase("metadata duplicate page uid", [{ path: "A.md", text: "- a\n" }, { path: "B.md", text: "- b\n" }], {
  version: 1,
  pages: { "A.md": { title: "A", uid: "same", children: [] }, "B.md": { title: "B", uid: "same", children: [] } },
});

// Round trips: import, then export the imported documents with and without edits.
const roundTrips: { name: string; input: unknown; edit?: (text: string) => string }[] = [
  { name: "rt: case-sensitive titles", input: JSON.parse(jsonImports[0].input) },
  { name: "rt: nested metadata", input: nested },
  { name: "rt: edited block", input: JSON.parse(jsonImports[8].input), edit: (text) => text.replace("- Original ^one", "- Edited ^one") },
  { name: "rt: multiline list", input: JSON.parse(jsonImports[9].input), edit: (text) => text.replace("- Before ^edited", "- After ^edited") },
  { name: "rt: unsupported IDs", input: JSON.parse(jsonImports[11].input) },
  { name: "macros and lists", input: JSON.parse(jsonImports.find((item) => item.name === "list markers and fences")!.input), edit: (text) => text.replace("- dash", "- DASH") },
  { name: "macros edited heading", input: JSON.parse(jsonImports.find((item) => item.name === "macros, images, tasks and headings")!.input), edit: (text) => text.replace("### third", "## third").replace("[ ] first", "[x] first") },
  { name: "attribute order", input: JSON.parse(jsonImports.find((item) => item.name === "attributes keep their order")!.input) },
  ...seededGraphs.map((input, index) => ({
    name: `seeded ${index}`,
    input,
    edit: index % 2 ? (text: string) => text.replace(/(\n {0,4}- )([^\n]*?)( \^)/, "$1edited $2$3") + (index % 4 === 1 ? "\nA new paragraph [[P0]]\n" : "") : undefined,
  })),
];
for (const { name, input, edit } of roundTrips) {
  resetRecording();
  let result;
  try {
    result = importRoamJSON(input);
  } catch {
    continue;
  }
  exportCase(`round trip ${name}`, result.documents, result.metadata, 1001);
  if (edit) exportCase(`round trip edited ${name}`, result.documents.map((doc) => ({ ...doc, text: edit(doc.text) })), result.metadata, 1001);
}

function run<T>(compute: () => T, firstId = 1) {
  resetRecording(firstId);
  try {
    return { result: compute(), error: null, ...recorded() };
  } catch (error) {
    return { result: null, error: message(error), ...recorded() };
  }
}

const output = {
  imports: jsonImports.map(({ name, input }) => ({ name, input, ...run(() => imported(importRoamJSON(JSON.parse(input)))) })),
  edn: ednImports.map(({ name, input }) => ({ name, input, ...run(() => imported(importRoamEDN(input))) })),
  reads: reads.map((input) => {
    try {
      return { input, output: JSON.stringify(ednJSON(readEDN(input))), error: null };
    } catch (error) {
      return { input, output: null, error: message(error) };
    }
  }),
  exports: exportsIn.map(({ name, documents, metadata, firstId }) => ({
    name,
    documents,
    metadata,
    firstId,
    ...run(() => {
      const exported = exportRoamJSON(documents, metadata === null ? undefined : (JSON.parse(metadata) as RoamMetadata));
      return { pages: JSON.stringify(exported.pages, null, 2) + "\n", report: exported.report };
    }, firstId),
  })),
};
writeFileSync("tests/fixtures/roam-vectors.json", JSON.stringify(output) + "\n");
const failed = (items: { error: string | null }[]) => items.filter((item) => item.error).length;
console.log(
  `Generated ${output.imports.length} JSON imports (${failed(output.imports)} refused), ${output.edn.length} EDN imports (${failed(output.edn)} refused), ${output.reads.length} EDN reads, and ${output.exports.length} exports (${failed(output.exports)} refused) from the frozen rt Roam modules.`,
);
