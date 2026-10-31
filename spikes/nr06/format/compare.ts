// Compares a tool's outputs with the Prettier reference outputs.
// Usage: bun compare.ts <out-dir> [--show]
import { readdirSync } from "node:fs";

const dir = process.argv[2];
const show = process.argv.includes("--show");
const ref = new URL("./expected/prettier/", import.meta.url).pathname;
const rows: { name: string; match: boolean; changed: number; error: boolean }[] = [];
for (const file of readdirSync(ref).sort()) {
  const expected = await Bun.file(ref + file).text();
  const f = Bun.file(`${dir}/${file}`);
  const actual = (await f.exists()) ? await f.text() : "ERROR: missing\n";
  const a = expected.split("\n"), b = actual.split("\n");
  // Changed lines: lines in either side without a match on the other (multiset difference).
  const count = (xs: string[]) => xs.reduce((m, x) => m.set(x, (m.get(x) ?? 0) + 1), new Map<string, number>());
  const ca = count(a), cb = count(b);
  let changed = 0;
  for (const [k, v] of ca) changed += Math.max(0, v - (cb.get(k) ?? 0));
  for (const [k, v] of cb) changed += Math.max(0, v - (ca.get(k) ?? 0));
  const error = actual.startsWith("ERROR");
  rows.push({ name: file, match: expected === actual, changed, error });
  if (show && expected !== actual) {
    console.log(`=== ${file}\n--- prettier\n${expected}--- ${dir}\n${actual}`);
  }
}
const lang = (n: string) => n.split(".").pop()!;
const groups = new Map<string, { total: number; match: number; errors: number; changed: number }>();
for (const r of rows) {
  const g = groups.get(lang(r.name)) ?? { total: 0, match: 0, errors: 0, changed: 0 };
  g.total++; if (r.match) g.match++; if (r.error) g.errors++; g.changed += r.changed;
  groups.set(lang(r.name), g);
}
console.log(JSON.stringify({
  tool: dir,
  identical: rows.filter((r) => r.match).length,
  total: rows.length,
  errors: rows.filter((r) => r.error).length,
  changed_lines: rows.reduce((s, r) => s + r.changed, 0),
  by_fence_tag: Object.fromEntries(groups),
  cases: Object.fromEntries(rows.map((r) => [r.name, r.match ? "identical" : r.error ? "error" : `${r.changed} lines differ`])),
}));
