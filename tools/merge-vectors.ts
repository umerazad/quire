import { writeFileSync } from "node:fs";
import { mergeText } from "../tests/reference/core/merge";
import { pageIdForPath, type Revision } from "../tests/reference/sync/format";
import { addRevision, createRevision, MemoryRevisions, mergeHeads } from "../tests/reference/sync/graph";
import { random } from "../tests/sync/harness";

// The page and edit generators are copied from rt tests/merge-go.test.ts at 5ea5c22310ff,
// so seed 42 yields the same 4,000 text cases rt checks against the Go terminal app.
const words = ["buy", "milk", "call", "Sam", "[ ]", "[x]", "#next", "^b-1", "^b-2", "اجتماع", "غدا", "[[Plan]]", "", "  ", "\t"];
function page(rng: () => number, lines: number) {
  const pick = <T>(items: T[]) => items[Math.floor(rng() * items.length)];
  const line = () =>
    pick(["", "  ", "    "]) +
    pick(["- ", "- [ ] ", "", "# "]) +
    Array.from({ length: Math.floor(rng() * 5) }, () => pick(words)).join(pick([" ", " ", "  "]));
  return Array.from({ length: lines }, line);
}
function edit(rng: () => number, base: string[]) {
  const out = [...base];
  const fresh = page(rng, 8);
  for (let n = Math.floor(rng() * 5); n >= 0; n--) {
    const at = Math.floor(rng() * (out.length + 1)),
      roll = rng();
    if (roll < 0.3) out.splice(at, 0, fresh[n % fresh.length]);
    else if (roll < 0.5) out.splice(at, 1);
    else if (roll < 0.9 && out.length) {
      const i = at % out.length,
        parts = out[i].split(" ");
      parts.splice(Math.floor(rng() * (parts.length + 1)), rng() < 0.5 ? 1 : 0, fresh[(n + 1) % fresh.length].trim());
      out[i] = parts.join(" ");
    } else out.splice(at, 0, ...fresh.slice(0, 3));
  }
  return out;
}

const textRng = random(42);
const generated = Array.from({ length: 4000 }, () => {
  const base = page(textRng, Math.floor(textRng() * 12));
  const ending = textRng() < 0.7 ? "\n" : "";
  return {
    base: base.join("\n") + ending,
    first: edit(textRng, base).join("\n") + ending,
    second: edit(textRng, base).join("\n") + (textRng() < 0.9 ? ending : ""),
  };
});
// Lone surrogates, CRLF, and astral characters exercise the UTF-16 code-unit contract.
const edges = [
  { base: "\ud800 a\n", first: "\ud800 b\n", second: "\udc00 a\n" },
  { base: "x\ud800\n", first: "x\ud800 one\n", second: "x\ud800 two\n" },
  { base: "a\r\nb\r\n", first: "a!\r\nb\r\n", second: "a\r\nb?\r\n" },
  { base: "🙂 ^id\n", first: "🙂 left ^id\n", second: "🙂 right ^id\n" },
  { base: "\udc00\n", first: "\ud800\n", second: "\udbff\n" },
  { base: "", first: "\ud83d", second: "\ude42" },
];
// First makes 2,500 edits, past the 2,000-edit cap, so its whole middle counts as replaced.
const long = Array.from({ length: 2500 }, (_, i) => `L${i}`);
edges.push({
  base: long.join("\n"),
  first: long.flatMap((line, i) => (i % 2 ? [] : [line, `N${i}`])).join("\n"),
  second: long.map((line, i) => (i === 1250 ? "changed" : line)).join("\n"),
});
const texts = [...generated, ...edges].map((c) => ({ ...c, ...mergeText(c.base, c.first, c.second) }));

const headRng = random(9001);
const pick = <T>(items: T[]) => items[Math.floor(headRng() * items.length)];
const paths = ["Plan.md", "Projects/Plan.md", "Archive/Plan 🙂.md", "\ud800.md"];
const devices = ["office", "home", "laptop"];
const heads = Array.from({ length: 300 }, (_, index) => {
  const deletion = index % 10 === 0 ? 0.9 : 0.15;
  const store = new MemoryRevisions();
  const id = pageIdForPath(`Page ${index}.md`);
  const revisions: Revision[] = [];
  const add = (parents: Revision[]) => {
    const from = parents[0];
    const lines = from ? from.text.split("\n") : page(headRng, Math.floor(headRng() * 8));
    const revision = createRevision(store, {
      page: id,
      parents: parents.map((parent) => parent.id),
      path: from && headRng() < 0.8 ? from.path : pick(paths),
      deleted: headRng() < deletion,
      text: (index % 50 === 0 ? "\udc00 " : "") + edit(headRng, lines).join("\n"),
      created: 1 + Math.floor(headRng() * 40),
      device: pick(devices),
    });
    if (addRevision(store, revision).kind === "added") revisions.push(revision);
  };
  add([]);
  for (let step = Math.floor(headRng() * 7); step >= 0; step--) {
    const parents = [pick(revisions)];
    if (headRng() < 0.2) parents.push(pick(revisions));
    add(parents);
  }
  while (store.heads(id).length < 2) add([pick(revisions)]);
  const merged = mergeHeads(store, id, { created: 5000, device: "merger" })!;
  const { created: _created, device: _device, ...expected } = merged.revision;
  return { revisions, expected, overlaps: merged.overlaps };
});

writeFileSync("tests/fixtures/merge-vectors.json", JSON.stringify({ texts, heads }) + "\n");
console.log(
  `Generated ${texts.length} text merges (${texts.filter((c) => c.overlaps).length} overlapping) and ${heads.length} head merges (${heads.filter((c) => c.expected.deleted).length} deleted, ${heads.filter((c) => c.expected.parents.length > 2).length} with three or more heads).`,
);
