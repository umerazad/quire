// Runs the NR34 performance workload on release builds and fails when a budget is exceeded.
// Usage: bun tools/bench/run.ts [--scales 1,5] [--launches 5] [--margin 0.1] [--features list]
//        [--skip-build] [--only-store]
// Fixtures come from tools/synthetic-notebook.ts (seed 1) plus a 1 MiB Large.md page, cached
// under target/bench/fixtures. Raw results go to target/bench/results/<time>.json.
import { cpSync, existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { loadavg } from "node:os";
import { join, resolve } from "node:path";

const root = resolve(import.meta.dir, "../..");
const argv = process.argv.slice(2);
const option = (name: string, fallback: string) => {
  const at = argv.indexOf(`--${name}`);
  return at >= 0 && argv[at + 1] ? argv[at + 1] : fallback;
};
const flag = (name: string) => argv.includes(`--${name}`);
const scales = option("scales", "1,5").split(",").map(Number);
const launches = Number(option("launches", "5"));
const config = JSON.parse(readFileSync(join(import.meta.dir, "budgets.json"), "utf8"));
const margin = Number(option("margin", String(config.margin)));
const features = option("features", process.platform === "darwin" ? "gpui_platform/runtime_shaders" : "");
const bench = join(root, "target/bench");
const BASE_PAGES = 6536;

async function run(command: string[], env: Record<string, string> = {}) {
  const child = Bun.spawn(command, { cwd: root, env: { ...process.env, ...env }, stdout: "pipe", stderr: "inherit" });
  const output = await new Response(child.stdout).text();
  const status = await child.exited;
  if (status !== 0) throw new Error(`${command.join(" ")} exited ${status}. Fix the reported error and rerun tools/bench/run.ts.`);
  return output;
}

if (!flag("skip-build")) {
  const featureArgs = features ? ["--features", features] : [];
  await run(["cargo", "build", "--release", "--locked", "-p", "quire", ...featureArgs]);
  await run(["cargo", "build", "--release", "--locked", "-p", "quire_store", "--example", "journal_bench", "--example", "index_queries"]);
}

function fixture(scale: number) {
  const folder = join(bench, "fixtures", `${BASE_PAGES * scale}-seed1`);
  if (existsSync(join(folder, "Large.md"))) return folder;
  rmSync(folder, { recursive: true, force: true });
  const generated = Bun.spawnSync(["bun", join(root, "tools/synthetic-notebook.ts"), folder, String(BASE_PAGES * scale), "1"], { stderr: "inherit" });
  if (generated.exitCode !== 0) throw new Error(`Cannot generate the ${scale}x fixture in ${folder}. Check free disk space and rerun.`);
  const lines: string[] = [];
  for (let i = 0, size = 0; size < 1 << 20; i++) {
    const line = `- Large block ${i} notebook outline review draft meeting project budget plan design storage index\n`;
    lines.push(line);
    size += line.length;
  }
  writeFileSync(join(folder, "Large.md"), lines.join(""));
  return folder;
}

type Report = Record<string, any> & { load: number[] };

async function launch(notebook: string, home: string, output: string): Promise<Report> {
  rmSync(output, { force: true });
  const load = loadavg();
  const child = Bun.spawn([join(root, "target/release/quire"), "--notebook-folder", notebook, "--page", "Topic 0.md"], {
    cwd: root,
    env: { ...process.env, HOME: home, QUIRE_BENCH: output, QUIRE_BENCH_SPAWNED_NS: String(BigInt(Date.now()) * 1_000_000n) },
    stdout: "ignore",
    stderr: Bun.file(`${output}.log`),
  });
  const timer = setTimeout(() => child.kill(), 600_000);
  await child.exited;
  clearTimeout(timer);
  if (!existsSync(output)) throw new Error(`Quire wrote no performance report within 10 minutes. Its error output ends with:\n${readFileSync(`${output}.log`, "utf8").slice(-2000)}`);
  const report = JSON.parse(readFileSync(output, "utf8"));
  if (report.error) throw new Error(`The performance run failed: ${report.error}`);
  return { ...report, load };
}

const percentile = (values: number[], p: number) => {
  const sorted = [...values].sort((a, b) => a - b);
  return sorted.length ? sorted[Math.min(sorted.length - 1, Math.ceil((p / 100) * sorted.length) - 1)] : NaN;
};
const stat = (values: number[], name: string) =>
  name === "max" ? Math.max(...values) : name === "value" ? values[0] : percentile(values, Number(name.slice(1)));

const metrics = new Map<string, number[]>();
const add = (scale: number, metric: string, values: number[]) =>
  metrics.set(`${metric}@${scale}`, [...(metrics.get(`${metric}@${scale}`) ?? []), ...values]);
const raw: Record<string, unknown> = { started: new Date().toISOString(), launches, margin, features, store: {} };

if (!flag("only-store")) {
  for (const scale of scales) {
    const source = fixture(scale);
    const work = join(bench, "work", `${scale}x`);
    rmSync(work, { recursive: true, force: true });
    mkdirSync(work, { recursive: true });
    const notebook = join(work, "notebook");
    cpSync(source, notebook, { recursive: true });
    // A short home keeps the single-instance socket path under the Unix limit of about 104 bytes.
    const home = mkdtempSync("/tmp/quire-bench-");
    const runs: Report[] = [];
    for (let index = 0; index <= launches; index++) {
      const report = await launch(notebook, home, join(work, `run-${index}.json`));
      runs.push(report);
      console.error(`${scale}x ${index ? "warm" : "first open"} ${index}: editable ${report.first_editable_ms} ms, index ${report.index_ready_ms} ms, unpainted ${report.unpainted}, load ${report.load.map((l) => l.toFixed(1)).join(" ")}`);
    }
    // The same queries without the app, on the index the launches left: less noise from the GUI.
    const states = join(home, process.platform === "darwin" ? "Library/Application Support/QuireNative" : ".local/share/QuireNative", "folder-notebooks");
    const [state] = existsSync(states) ? readdirSync(states) : [];
    raw[`${scale}x_store_queries`] = state
      ? JSON.parse(await run([join(root, "target/release/examples/index_queries"), notebook, join(states, state)]))
      : "no folder state found; check where this platform keeps QuireNative data";
    rmSync(home, { recursive: true, force: true });
    raw[`${scale}x`] = runs;
    const [first, ...warm] = runs;
    add(scale, "first_open_first_editable_ms", [first.first_editable_ms]);
    add(scale, "first_open_index_ready_ms", [first.index_ready_ms]);
    add(scale, "warm_first_editable_ms", warm.map((r) => r.first_editable_ms));
    add(scale, "warm_index_ready_ms", warm.map((r) => r.index_ready_ms));
    for (const r of runs) {
      for (const metric of ["typing_work_ms", "typing_frame_ms", "page_open_ms", "search_ms", "backlinks_ms", "tasks_ms", "tags_ms"]) add(scale, metric, r[metric]);
      add(scale, "resident_mib_after_opens", [r.resident_kib_after_opens / 1024]);
      add(scale, "resident_mib_at_end", [r.resident_kib_at_end / 1024]);
      if (r.large_page) {
        add(scale, "large_open_ms", [r.large_page.open_ms]);
        add(scale, "large_typing_work_ms", r.large_page.typing_work_ms);
        add(scale, "large_typing_frame_ms", r.large_page.typing_frame_ms);
      }
    }
  }
  const warm = (scale: number) => percentile(metrics.get(`warm_first_editable_ms@${scale}`) ?? [], 50);
  const resident = (scale: number) => Math.max(...(metrics.get(`resident_mib_after_opens@${scale}`) ?? [NaN]));
  if (scales.includes(1) && scales.includes(5)) {
    metrics.set("warm_startup_ratio_5x_1x@5", [warm(5) / warm(1)]);
    metrics.set("resident_growth_mib_5x_1x@5", [resident(5) - resident(1)]);
  }
}

const journal = await run([join(root, "target/release/examples/journal_bench"), "1000", bench]);
const imported = journal.match(/^import: \d+ in [\d.]+ s, ([\d.]+) ms each/m);
if (!imported) throw new Error(`Cannot read journal_bench's import line from:\n${journal}`);
metrics.set("folder_import_ms_per_note@0", [Number(imported[1])]);
raw.store = { journal_bench: journal.trim().split("\n") };

let failed = 0;
const rows = config.budgets.flatMap((budget: { metric: string; stat: string; scale: number; limit: number }) => {
  const values = metrics.get(`${budget.metric}@${budget.scale}`);
  if (!values?.length) return [];
  const value = stat(values, budget.stat);
  const ok = value <= budget.limit * (1 + margin);
  if (!ok) failed++;
  return [{ metric: budget.metric, scale: budget.scale ? `${budget.scale}x` : "-", stat: budget.stat, value: Math.round(value * 100) / 100, limit: budget.limit, used: `${Math.round((value / budget.limit) * 100)}%`, ok: ok ? "pass" : "FAIL" }];
});
console.table(rows);
raw.summary = rows;
raw.metrics = Object.fromEntries(metrics);
mkdirSync(join(bench, "results"), { recursive: true });
const saved = join(bench, "results", `${new Date().toISOString().replace(/[:.]/g, "-")}.json`);
writeFileSync(saved, JSON.stringify(raw, null, 1));
console.error(`Raw measurements: ${saved}. Load average now ${loadavg().map((l) => l.toFixed(1)).join(" ")}.`);
if (failed) {
  console.error(`${failed} budget(s) exceeded by more than ${margin * 100}%. Profile the failing measure, fix it, and rerun; change a budget only through a recorded tradeoff in plans/native-rewrite.md.`);
  process.exit(1);
}
