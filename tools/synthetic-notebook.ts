// Writes a deterministic synthetic notebook for storage benchmarks.
// Usage: bun scripts/synthetic-notebook.ts <empty folder> [pages] [seed]
// The default size matches the largest real notebook measured so far:
// 6,536 pages and about 12.7 MB of Markdown.
import { mkdirSync, readdirSync, writeFileSync, existsSync } from "node:fs";
import path from "node:path";

const [target, pagesArg = "6536", seedArg = "1"] = process.argv.slice(2);
if (!target)
  throw new Error(
    "Pass an empty folder: bun scripts/synthetic-notebook.ts <folder> [pages] [seed]",
  );
if (existsSync(target) && readdirSync(target).length)
  throw new Error(`${target} is not empty. Choose a new or empty folder.`);
const pageCount = Number(pagesArg);
if (!Number.isInteger(pageCount) || pageCount < 10)
  throw new Error("Pass a page count of at least 10.");

let state = Number(seedArg) >>> 0 || 1;
const random = () => {
  state ^= state << 13;
  state ^= state >>> 17;
  state ^= state << 5;
  return (state >>> 0) / 2 ** 32;
};
const int = (max: number) => Math.floor(random() * max);
const pick = <T>(items: readonly T[]) => items[int(items.length)];

const words =
  "notebook outline thought review draft meeting project vendor budget plan design storage index search query result reference context decision question answer follow call email schedule week month quarter release customer support research summary detail".split(
    " ",
  );
const arabic = "ملاحظات الاجتماع خطة المشروع مراجعة الأسبوع";
const urdu = "میٹنگ کے نوٹس منصوبہ ہفتہ وار جائزہ";
const tags = Array.from({ length: 200 }, (_, i) => `topic-${i}`);
const states = ["inbox", "next", "waiting", "someday"];

const day = (offset: number) => {
  const date = new Date(Date.UTC(2023, 0, 1) + offset * 86_400_000);
  return date.toISOString().slice(0, 10);
};
const dailyCount = Math.round(pageCount * 0.25),
  peopleCount = Math.round(pageCount * 0.05),
  topicCount = pageCount - dailyCount - peopleCount;
const titles = [
  ...Array.from({ length: dailyCount }, (_, i) => ({
    title: day(i),
    file: `${day(i)}.md`,
  })),
  ...Array.from({ length: peopleCount }, (_, i) => ({
    title: `Person ${i}`,
    file: `People/Person ${i}.md`,
  })),
  ...Array.from({ length: topicCount }, (_, i) => ({
    title: `Topic ${i}`,
    file: `Topic ${i}.md`,
  })),
];
const blockIds: string[] = [];
const sentence = () =>
  Array.from({ length: 6 + int(14) }, () => pick(words)).join(" ");
function block(depth: number, page: number, index: number) {
  const parts = [sentence()];
  if (random() < 0.35) parts.push(`[[${pick(titles).title}]]`);
  if (random() < 0.25) parts.push(`#${pick(tags)}`);
  if (random() < 0.03) parts.push(random() < 0.5 ? arabic : urdu);
  if (blockIds.length && random() < 0.03) parts.push(`((${pick(blockIds)}))`);
  let text = parts.join(" ");
  if (random() < 0.08) {
    const done = random() < 0.3;
    text = `[${done ? "x" : " "}] ${text} #${pick(states)}`;
    if (random() < 0.5) text += ` 📅 [[${day(int(dailyCount + 60))}]]`;
  }
  if (random() < 0.04) {
    const id = `b${page}-${index}`;
    blockIds.push(id);
    text += ` ^${id}`;
  }
  if (blockIds.length && random() < 0.005)
    text = `{{embed: ((${pick(blockIds)}))}}`;
  return `${"  ".repeat(depth)}- ${text}\n`;
}
let bytes = 0;
const folders = new Set<string>();
titles.forEach(({ file }, page) => {
  const blocks = 5 + int(28);
  let depth = 0,
    text = "";
  for (let i = 0; i < blocks; i++) {
    text += block(depth, page, i);
    depth = Math.max(0, Math.min(4, depth + (random() < 0.4 ? 1 : 0) - int(2)));
  }
  const full = path.join(target, file),
    folder = path.dirname(full);
  if (!folders.has(folder)) {
    mkdirSync(folder, { recursive: true });
    folders.add(folder);
  }
  writeFileSync(full, text);
  bytes += Buffer.byteLength(text);
});
console.log(
  JSON.stringify({
    pages: titles.length,
    daily: dailyCount,
    people: peopleCount,
    topics: topicCount,
    blockIds: blockIds.length,
    markdownBytes: bytes,
  }),
);
