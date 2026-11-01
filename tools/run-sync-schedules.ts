import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

if (process.platform === "win32") throw new Error("The schedule transport needs Unix FIFOs. Run it on macOS or Linux; Windows durability remains a separate gate.");
const root = resolve(import.meta.dir, "..");
const build = Bun.spawn(["cargo", "build", "-p", "quire_store", "--example", "replica_probe", "--locked"], { cwd: root, stdout: "inherit", stderr: "inherit" });
const buildStatus = await build.exited;
if (buildStatus !== 0) process.exit(buildStatus);
const metadata = Bun.spawn(["cargo", "metadata", "--no-deps", "--format-version", "1"], { cwd: root, stdout: "pipe", stderr: "inherit" });
const metadataText = await new Response(metadata.stdout).text();
const metadataStatus = await metadata.exited;
if (metadataStatus !== 0) process.exit(metadataStatus);
const binary = join(JSON.parse(metadataText).target_directory, "debug/examples/replica_probe");
const directory = mkdtempSync(join(tmpdir(), "quire-schedule-runner-"));
const runtime = process.argv.indexOf("--runtime");
const env: Record<string, string | undefined> = { ...process.env, QUIRE_REPLICA_PROBE: binary };
let status = 1;
try {
  if (runtime >= 0 && process.argv[runtime + 1] === "go") {
    // The Go terminal app comes from an rt checkout, built offline from its module cache.
    const rt = resolve(process.env.RT_CHECKOUT ?? join(root, "../rt"));
    const go = Bun.spawnSync(["go", "build", "-o", join(directory, "quire-go"), "./cmd/quire"], {
      cwd: join(rt, "tui"), env: { ...process.env, GOFLAGS: "-mod=readonly", GOPROXY: "off" }, stdout: "inherit", stderr: "inherit",
    });
    if (go.exitCode !== 0) throw new Error(`Cannot build the Go terminal app in ${rt}/tui. Install Go, set RT_CHECKOUT to an rt checkout whose module cache is filled, and retry.`);
    env.QUIRE_GO_PEER = join(directory, "quire-go");
  }
  const result = await Bun.build({ entrypoints: [join(root, "tests/sync/run-schedules.ts")], target: "node", outdir: directory, naming: "schedules.mjs" });
  if (!result.success) throw new AggregateError(result.logs, "Cannot bundle the schedule runner. Fix the reported imports and retry.");
  const child = Bun.spawn(["node", join(directory, "schedules.mjs"), ...process.argv.slice(2)], {
    cwd: root, env, stdin: "inherit", stdout: "inherit", stderr: "inherit",
  });
  status = await child.exited;
} finally {
  rmSync(directory, { recursive: true, force: true });
}
process.exit(status);
