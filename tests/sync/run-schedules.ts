import assert from "node:assert/strict";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { check, runSchedule } from "./harness";
import { NativeProbe, nativeEngine } from "./native-peer";
import { focusedSchedules } from "./focused";
import { editOnePageAtOnce } from "./editors";
import { closeGoPeers, goEngine } from "./go-peer";
import { goHandover } from "./go-focused";

const args = process.argv.slice(2);
function option(name: string, fallback: string) {
  const at = args.indexOf(name);
  if (at < 0) return fallback;
  assert(at + 1 < args.length, `${name} needs a value`);
  return args[at + 1];
}
for (let i = 0; i < args.length; i += 2)
  assert(["--from", "--count", "--steps", "--runtime"].includes(args[i]), `Unknown option ${args[i]}`);
const from = Number(option("--from", "1")), count = Number(option("--count", "10000")), steps = Number(option("--steps", "60"));
assert([from, count, steps].every((value) => Number.isSafeInteger(value) && value > 0), "Seed, count, and steps must be positive integers");
assert(from + count <= 2 ** 32, "Seeds must fit the scheduler's uint32 state");
const runtime = option("--runtime", "both");
assert(["both", "native", "mixed", "alternating", "go"].includes(runtime), "Runtime must be both, native, mixed, alternating, or go");
const runtimes = (seed: number) => runtime === "both" ? ["native", "mixed"] : runtime === "alternating" ? [seed % 2 === 0 ? "native" : "mixed"] : [runtime];
const goBinary = process.env.QUIRE_GO_PEER;
assert(runtime !== "go" || goBinary, "QUIRE_GO_PEER must point to the Go terminal app built from the rt checkout; run through bun run test:sync-schedules");
const binary = process.env.QUIRE_REPLICA_PROBE;
assert(binary, "QUIRE_REPLICA_PROBE must point to the compiled native probe");
const directory = mkdtempSync(join(tmpdir(), "quire-schedules-"));
console.log(`Schedule fixtures: ${directory}`);
const probe = new NativeProbe(binary, directory);
const started = performance.now();
const completed = { native: 0, mixed: 0, go: 0 };
const operations: Record<string, number> = {};
let active = { seed: from, runtime: runtimes(from)[0] };
let passed = false;
let lastProgress = performance.now();
try {
  let focused = await focusedSchedules(probe);
  // A native editor and rt's TypeScript replica editor on one page at once, in both seats, and
  // two native editors.
  for (const [office, home] of [[true, false], [false, true], [true, true]]) {
    for (let seed = 1; seed <= 20; seed++) {
      probe.request({ op: "reset" });
      await editOnePageAtOnce(seed, office ? nativeEngine(probe, "office") : undefined, home ? nativeEngine(probe, "home") : undefined);
    }
    focused++;
  }
  if (runtime === "go") {
    probe.request({ op: "reset" });
    await goHandover(nativeEngine(probe, "home"), goEngine(goBinary!, directory, "office", "sqlite"));
    closeGoPeers();
    for (let seed = 1; seed <= 20; seed++) {
      probe.request({ op: "reset" });
      await editOnePageAtOnce(seed, goEngine(goBinary!, directory, "office", seed % 2 ? "memory" : "sqlite"), nativeEngine(probe, "home"));
      closeGoPeers();
    }
    focused += 2;
  }
  console.log(JSON.stringify({ focused }));
  for (let seed = from; seed < from + count; seed++) {
    for (const kind of runtimes(seed)) {
      active = { seed, runtime: kind };
      probe.trace = [];
      probe.request({ op: "reset" });
      // Go runs put the Go terminal app in the office, on SQLite every tenth seed, among native computers.
      const engine = (name: string) => kind === "go"
        ? name === "office" ? goEngine(goBinary!, directory, name, seed % 10 === 0 ? "sqlite" : "memory") : nativeEngine(probe, name)
        : kind === "native" || name === "office" ? nativeEngine(probe, name) : undefined;
      const run = await runSchedule(seed, steps, undefined, engine);
      assert.equal(check(run), undefined, `seed ${seed}, ${kind}, ${run.mode}`);
      // IDs hash semantic content. Compare fields explicitly as well as presence and ancestry.
      for (const computer of run.computers) {
        for (const saved of run.saved.values()) {
          const actual = computer.store.get(saved.id)!;
          for (const field of ["id", "page", "parents", "generation", "path", "text", "deleted"] as const)
            assert.deepEqual(actual[field], saved[field], `seed ${seed}, ${kind}, ${computer.name}, ${saved.id}, ${field}`);
        }
      }
      closeGoPeers();
      for (const item of probe.trace as { request?: { op: string } }[])
        if (item.request) operations[item.request.op] = (operations[item.request.op] ?? 0) + 1;
      completed[kind as keyof typeof completed]++;
    }
    if (performance.now() - lastProgress >= 5000 || seed === from + count - 1) {
      console.log(JSON.stringify({ seed, completed, seconds: Math.round((performance.now() - started) / 1000), requests: probe.requests }));
      lastProgress = performance.now();
    }
  }
  passed = true;
  console.log(JSON.stringify({ result: "passed", from, count, steps, focused, completed, operations, requests: probe.requests, seconds: (performance.now() - started) / 1000, node: process.version }));
} catch (error) {
  writeFileSync(join(directory, "failure.json"), JSON.stringify({ ...active, steps, error: String(error), trace: probe.trace }, null, 2));
  console.error(`Replay: bun run test:sync-schedules --from ${active.seed} --count 1 --steps ${steps} --runtime ${active.runtime}`);
  console.error(`Retained native databases, folder views, and request trace: ${directory}`);
  throw error;
} finally {
  closeGoPeers();
  probe.close();
  if (passed) rmSync(directory, { recursive: true, force: true });
}
