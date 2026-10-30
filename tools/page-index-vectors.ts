import { readFileSync, writeFileSync } from "node:fs";
import { editorText } from "../tests/reference/markdown/markdown";
import { analyzePage } from "../tests/reference/index/page-analysis";

const fixture = (name: string) =>
  JSON.parse(readFileSync(new URL(`../tests/fixtures/markdown/${name}`, import.meta.url), "utf8"));
const sources: { name: string; source: string }[] = [
  ...fixture("inputs.json").map((input: { name: string; source: string }) => ({ name: `markdown-${input.name}`, source: input.source })),
  ...fixture("tag-vectors.json").map((vector: { name: string; raw: { text: string } }) => ({ name: `tag-${vector.name}`, source: vector.raw.text })),
  ...fixture("reference-vectors.json").map((vector: { name: string; raw: { text: string } }) => ({ name: `reference-${vector.name}`, source: vector.raw.text })),
  { name: "frontmatter-tags", source: "---\ntags: [one, \"two\"]\n---\n- a #x ^id1\n  - b [[Page]] #x #y\n- c ((id1)) #X\n" },
  { name: "nested-links", source: "# Title\n\n- parent [[P]]\n  - child ((c1)) ^c2\n    - grandchild ![[E]]\n\nPara [[Q|label]] #t\n" },
];
const vectors = sources.map(({ name, source }) => {
  const { blocks, links, tagged, frontTags } = analyzePage("page.md", editorText(source));
  return {
    name,
    source,
    blocks: blocks.map(({ key, id, from, to, contentFrom, contentTo, subtreeTo, depth, kind, parent, text }) => ({ key, id: id ?? null, from, to, contentFrom, contentTo, subtreeTo, depth, kind, parent: parent ?? null, text })),
    links: links.map(({ kind, syntax, target, label, media, from, to, text }) => ({ kind, syntax, target, label: label ?? null, media: media ?? null, from, to, text })),
    tagged: tagged.map(({ tag, from, blockId, snippet }) => ({ tag, from, blockId: blockId ?? null, snippet })),
    frontTags,
  };
});
writeFileSync(new URL("../tests/fixtures/markdown/page-index-vectors.json", import.meta.url), JSON.stringify(vectors, null, 1) + "\n");
console.log(`Wrote ${vectors.length} page index vectors.`);
