import { execFileSync } from "node:child_process";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { dailyPath, dailyTitle, detectDailyLayout, parseDaily, setActiveDailyLayout } from "../tests/reference/daily/daily-layout";
import { dailyDate } from "../tests/reference/daily/workspace";
import { localDate } from "../tests/reference/daily/local-date";

if (process.argv.includes("--probe")) {
  const layouts = [
    {folder: "", title: "{YYYY}-{MM}-{DD}"},
    {folder: "Daily/{YYYY}/{MM}-{MMMM}", title: "{YYYY}-{MM}-{DD}-{dddd}"},
    {folder: "", title: "{DD} {MMMM} {YYYY}"},
  ];
  const dates = ["2011-12-29", "2011-12-30", "2011-12-31", "1892-07-04", "1993-08-21", "2018-11-04", "2026-03-08", "2026-11-01", "2026-10-29", "0000-02-29", "0099-01-01", "9999-12-31"];
  const formatted = layouts.flatMap(layout => dates.map(iso => {
    setActiveDailyLayout(layout);
    const title = dailyTitle(iso, layout);
    return {layout, iso, title, path: dailyPath(iso, layout), parsed: parseDaily(title, layout.title) ?? null, date: dailyDate(title) ?? null};
  }));
  const titles = [...dates, "2011-12-30-Friday", "2011-12-30-Saturday", "December 30th, 2011", "August 21st, 1993", "November 4th, 2018", "July 4th, 1892"];
  const parsed = layouts.flatMap(layout => titles.map(title => { setActiveDailyLayout(layout); return {layout, title, date: dailyDate(title) ?? null}; }));
  const clocks = ["2011-12-30T09:59:59.999Z", "2011-12-30T10:00:00Z", "2026-10-29T21:30:00Z", "2026-03-08T06:59:59Z", "2026-03-08T07:00:00Z", "2026-11-01T05:30:00Z", "2026-11-01T06:30:00Z"].map(iso => {
    const date = new Date(iso); return {milliseconds: date.getTime(), date: localDate(date)};
  });
  const paths = ["Daily/2011-12-30-Friday.md", "Daily/2011-12-30-Saturday.md", "Other/2011-12-31.md"];
  process.stdout.write(JSON.stringify({zone: process.env.TZ, formatted, parsed, clocks, detection: {paths, layout: detectDailyLayout(paths) ?? null}}));
} else {
  generate().catch(error => { console.error(error); process.exitCode = 1; });
}

async function generate() {
  const root = mkdtempSync(join(tmpdir(), "quire-daily-timezones-"));
  try {
    const output = join(root, "probe.cjs");
    const built = await Bun.build({entrypoints: [import.meta.path], target: "node", format: "cjs", outdir: root, naming: "probe.cjs"});
    if (!built.success) throw new Error(`Cannot bundle the daily oracle. Fix build errors before regenerating fixtures: ${built.logs.join("\n")}`);
    const zones = ["UTC", "Asia/Riyadh", "America/New_York", "Australia/Lord_Howe", "Pacific/Apia", "Pacific/Kwajalein", "America/Sao_Paulo", "Europe/Paris"];
    const cases = zones.map(zone => JSON.parse(execFileSync("node", [output, "--probe"], {env: {...process.env, TZ: zone}, encoding: "utf8"})));
    writeFileSync("tests/fixtures/daily-timezone-vectors.json", JSON.stringify(cases, null, 2) + "\n");
    console.log(`Generated daily date/clock vectors for ${zones.length} isolated Node timezones.`);
  } finally { rmSync(root, {recursive: true, force: true}); }
}
