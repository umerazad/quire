import { writeFileSync } from "node:fs";
import { dailyFolder, dailyPath, dailyTitle, detectDailyLayout, formatDaily, layoutProblem, parseDaily, readNotebookSettings, setActiveDailyLayout, writeNotebookSettings } from "../tests/reference/daily/daily-layout";
import { dailyDate } from "../tests/reference/daily/workspace";

const layouts = [
  { folder: "", title: "{YYYY}-{MM}-{DD}" },
  { folder: "Extras/Calendar/{YYYY}/{MM}-{MMMM}", title: "{YYYY}-{MM}-{DD}-{dddd}" },
  { folder: "/Daily//{YYYY}/", title: "{DD} {MMMM} {YYYY}" },
  { folder: "اردو/{YYYY}", title: "🙂 {YYYY} [.{MM}.] {DD} {dddd}" },
  { folder: "", title: "{YYYY}{MM}{DD}" },
  { folder: "", title: "{YYYY}-{MM}-{DD} {MM} {MMMM} {dddd}" },
  { folder: "", title: "Σ k S {YYYY}-{MM}-{DD}" },
  { folder: "", title: "{MMMM}{dddd} {DD} {YYYY}" },
];
const dates = ["2024-02-29", "1900-02-28", "2000-02-29", "2025-12-31", "2026-01-01", "2026-10-29", "0099-01-01", "0000-02-29", "0100-03-01", "9999-12-31"];
let state = 0x7139a5;
const random = (n: number) => { state ^= state << 13; state ^= state >>> 17; state ^= state << 5; return (state >>> 0) % n; };
for (let index = 0; index < 300; index++) dates.push(`${1000 + random(9000)}-${String(1 + random(12)).padStart(2, "0")}-${String(1 + random(28)).padStart(2, "0")}`);
const formatted = layouts.flatMap(layout => dates.map(iso => {
  const title = dailyTitle(iso, layout);
  return { layout, iso, title, folder: dailyFolder(iso, layout), path: dailyPath(iso, layout), parsed: parseDaily(title, layout.title) ?? null };
}));
const parsed = layouts.flatMap(layout => [
  ...formatted.filter(value => value.layout === layout).slice(0, 10).flatMap(value => [value.title, value.title.toLowerCase(), value.title.toUpperCase(), value.title + "\n", value.title.replace("Thursday", "Friday"), value.title.replace("k", "K").replace("S", "ſ")]),
  "2026-02-30", "2026-13-01", "2026-00-01", "2026-01-00", "Research 2026", "", "0000-01-01", "2024-02-29", "29 October 2026", "2026-10-29-Thursday", "2026-10-29-Friday",
  ...["January", "Jan", "Januarysuffix", "May", "Sept", "Foo"].flatMap(month => ["0", "1", "29", "30", "31", "32"].flatMap(day => ["0000", "0049", "0050", "0099", "0100", "2026"].map(year => `${month} ${day}th, ${year}`))),
].map(title => { setActiveDailyLayout(layout); return { layout, title, parsed: parseDaily(title, layout.title) ?? null, date: dailyDate(title) ?? null }; }));
const problems = [...layouts, ...["..", ".hidden", "a/.x", "/out", "a\\b", "a//b", "a/../b"].map(folder => ({ folder, title: "{YYYY}-{MM}-{DD}" })), ...["{YYYY}-{MM}", "{YYYY}-{DD}", "{YYYY}/{MM}/{DD}", "{YYYY}\\{MMMM}-{DD}", "{YYYY}-{MMMM}-{DD}", "{yyyy}-{MM}-{DD}"].map(title => ({folder: "", title}))].map(layout => ({layout, problem: layoutProblem(layout) ?? null, written: writeNotebookSettings(layout)}));
const settings = [undefined, "", "{", "null", "[]", "true", '{"version":2,"daily":{}}', '{"version":"1"}', '{"version":1}', ...[null, false, 0, "", "literal", [], {}, {folder: 3, title: null}, {folder: "../out"}, {title: "broken"}].map(daily => JSON.stringify({version: 1, daily})), ...layouts.map(writeNotebookSettings)].map(text => ({text: text ?? null, result: readNotebookSettings(text)}));
const detections = [[], ["2026-10-29.md"], ["A/2026-10-29-Thursday.md", "B/2026-10-30-Friday.md"], ["Daily/2026/10-October/2026-10-29-Thursday.md", "Daily/2026/11-November/2026-11-01-Sunday.MD", "Other.md"], ["10/1010/210/2026-10-29.md"], ["2026-10-29-Friday.md", "A/2026-02-30.md"], ["B/2026-10-29.md", "A/2026-10-30.md", "A/2026-10-31.md"]].map(paths => ({paths, layout: detectDailyLayout(paths) ?? null}));
writeFileSync("tests/fixtures/daily-vectors.json", JSON.stringify({formatted, parsed, problems, settings, detections}, null, 2) + "\n");
console.log(`Generated ${formatted.length + parsed.length + problems.length + settings.length + detections.length} daily vectors.`);
