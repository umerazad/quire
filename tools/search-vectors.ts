import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { SearchIndex } from "../tests/reference/index/search-index";

// Simple block shapes only: NR10.8 tracks the native block-boundary gaps separately.
const words = ["alpha", "beta", "gamma", "delta", "plan", "planning", "planner", "road-map", "q3", "2026", "x", "مرحبا", "اردو", "café", "cafe", "naïve", "über", "東京", "emoji🙂", "re_use", "meeting", "notes", "Project", "PROJECT", "data", "database"];
let state = 0x5eed1;
const random = (length: number) => { state ^= state << 13; state ^= state >>> 17; state ^= state << 5; return (state >>> 0) % length; };
const sentence = () => Array.from({ length: 2 + random(7) }, () => (random(5) === 0 ? words[random(words.length)] : `filler${random(400)}`)).join(" ");
const pages: { path: string; text: string }[] = [];
for (let index = 0; index < 60; index++) {
  const folder = ["", "notes/", "daily/", "projects/"][random(4)];
  const lines: string[] = [];
  if (random(3) === 0) lines.push("---", `title: ${sentence()}`, "---");
  for (let block = 0; block < 1 + random(6); block++) {
    const shape = random(4);
    const id = random(4) === 0 ? ` ^id-${index}-${block}` : "";
    if (shape === 0) lines.push(`# ${sentence()}${id}`, "");
    else if (shape === 1) lines.push(`${sentence()} ${sentence()}${id}`, "");
    else lines.push(`- ${sentence()}${id}`, ...(random(2) ? [`  - ${sentence()}`] : []), "");
  }
  pages.push({ path: `${folder}page ${index} ${words[random(words.length)]}.md`, text: lines.join("\n") });
}
const queries = ["filler1", "filler", "fill", "alpha", "plan", "PLAN", "plan beta", "road-map", "road", "q3", "2026", "x", "مرحبا", "اردو", "café", "cafe", "naive", "über", "東京", "emoji", "re_use", "project", "data", "da", "a", "", "   ", "!!!", "alpha -beta", "\"plan\"", "meeting notes", "notes meeting", "zzz"];
const directory = mkdtempSync(join(tmpdir(), "quire-search-"));
try {
  const index = new SearchIndex(join(directory, "search.sqlite"));
  index.reconcile(pages);
  // The same statement as SearchIndex.search, plus bm25 so ties can be compared as sets.
  const scores = (index as unknown as { db: import("node:sqlite").DatabaseSync }).db.prepare(
    "SELECT bm25(notes) AS score FROM notes WHERE notes MATCH ? ORDER BY bm25(notes) LIMIT 40",
  );
  const vectors = queries.map((query) => {
    const hits = index.search(query);
    const terms = query.match(/[\p{L}\p{N}_-]+/gu) ?? [];
    const ranked = terms.length ? scores.all(terms.map((term) => `"${term}"*`).join(" AND ")) : [];
    return { query, hits: hits.map((hit, position) => ({ path: hit.path, title: hit.title, snippet: hit.snippet, blockId: hit.blockId ?? null, score: Number(ranked[position].score) })) };
  });
  index.close();
  writeFileSync(new URL("../tests/fixtures/search-vectors.json", import.meta.url), JSON.stringify({ pages, queries: vectors }, null, 1) + "\n");
  console.log(`Wrote ${pages.length} pages and ${vectors.length} queries (${vectors.reduce((sum, vector) => sum + vector.hits.length, 0)} hits).`);
} finally {
  rmSync(directory, { recursive: true, force: true });
}
