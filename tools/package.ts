// Helpers shared by the packaging scripts (bundle-mac.ts, bundle-linux.ts, bundle-windows.ts,
// verify-bundle-mac.ts).
import { createHash } from "node:crypto";
import { existsSync, lstatSync, mkdtempSync, readdirSync, readFileSync, rmSync, statSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, relative, resolve } from "node:path";
import { spawnSync, type SpawnSyncOptions } from "node:child_process";
import { archives, build as pdfiumBuild, cacheDir, libraryPath } from "./pdfium.ts";

export const root = resolve(import.meta.dirname, "..");

// The identity users and the OS see. rt ships as "Quire" with bundle id app.quire.desktop and the
// Go terminal app installs `quire`; the native build stays distinct from both until cutover.
export const identity = {
  name: "Quire Native",
  bundleId: "app.quire.native",
  rtBundleId: "app.quire.desktop",
  command: "quire-native",
  macExecutable: "quire",
  minimumMacOS: "12.0",
} as const;

export const licensesFile = "THIRD-PARTY-LICENSES.md";
export const resources = join(root, "crates/quire/resources");

export function fail(message: string): never {
  console.error(message);
  process.exit(1);
}

export function run(command: string, args: string[], options: SpawnSyncOptions = {}): string {
  const result = spawnSync(command, args, { cwd: root, encoding: "utf8", maxBuffer: 1 << 30, ...options });
  if (result.error) fail(`${command} could not start (${result.error.message}). Check that it is installed and on PATH.`);
  if (result.status !== 0)
    fail(`${command} ${args.join(" ")} failed with exit ${result.status}.\n${result.stderr ?? ""}\nFix the error above, then rerun.`);
  return String(result.stdout ?? "");
}

export function quireVersion(): string {
  const manifest = readFileSync(join(root, "crates/quire/Cargo.toml"), "utf8");
  const version = /^version = "([^"]+)"/m.exec(manifest)?.[1];
  if (!version) fail("crates/quire/Cargo.toml has no package version. Restore its `version = ...` line.");
  return version;
}

export function sha256(path: string): string {
  return createHash("sha256").update(readFileSync(path)).digest("hex");
}

// Every regular file and symbolic link under `dir`, as sorted paths relative to it.
export function filesUnder(dir: string): string[] {
  const walk = (at: string): string[] =>
    readdirSync(at)
      .sort()
      .flatMap((name) => {
        const path = join(at, name);
        return lstatSync(path).isDirectory() ? walk(path) : [path];
      });
  return walk(dir).map((path) => relative(dir, path));
}

export function hashTree(dir: string): Record<string, string> {
  return Object.fromEntries(
    filesUnder(dir).map((file) => {
      const path = join(dir, file);
      return [file, lstatSync(path).isSymbolicLink() ? "symlink" : sha256(path)];
    }),
  );
}

export function treeBytes(dir: string): number {
  return filesUnder(dir).reduce((sum, file) => sum + lstatSync(join(dir, file)).size, 0);
}

export function pdfium(platform: string): { dir: string; library: string } {
  const library = libraryPath(platform);
  if (!existsSync(library))
    fail(`PDFium for ${platform} is missing at ${library}. Run \`bun tools/fetch-pdfium.ts ${platform}\`, then rerun.`);
  return { dir: cacheDir(platform), library };
}

export function megabytes(bytes: number): string {
  return `${(bytes / 1_000_000).toFixed(1)} MB`;
}

// What went into a package, so a later build or the verifier can tell whether the inputs changed.
export function inputs(options: { target: string; features: string[]; platform: string; library: string }) {
  const dirty = run("git", ["status", "--porcelain", "--untracked-files=no"]).trim() !== "";
  return {
    version: quireVersion(),
    commit: run("git", ["rev-parse", "HEAD"]).trim(),
    dirty,
    rustc: run("rustc", ["-vV"]).trim(),
    target: options.target,
    features: options.features,
    cargoLock: sha256(join(root, "Cargo.lock")),
    cargoToml: sha256(join(root, "Cargo.toml")),
    resources: hashTree(resources),
    pdfium: {
      build: pdfiumBuild,
      platform: options.platform,
      archiveSha256: archives[options.platform],
      librarySha256: sha256(options.library),
    },
  };
}

export function isFile(path: string): boolean {
  return existsSync(path) && statSync(path).isFile();
}

// A packaged `--mcp-server` must answer with no app running, as rt's scripts/package-smoke.cjs
// checks. Returns the problem, or undefined when the server behaves.
const closedApp = "Quire is not running with a notebook open. Open Quire with the notebook, then retry.";

export function mcpServer(executable: string): string | undefined {
  const scratch = mkdtempSync(join(tmpdir(), "quire-verify-"));
  try {
    const frames = [
      { id: 1, method: "initialize", params: { protocolVersion: "2025-06-18" } },
      { id: 2, method: "tools/list" },
      { id: 3, method: "tools/call", params: { name: "tasks", arguments: {} } },
    ].map((frame) => JSON.stringify({ jsonrpc: "2.0", ...frame }));
    const result = spawnSync(executable, ["--mcp-server"], {
      encoding: "utf8",
      input: `${frames.join("\n")}\n`,
      timeout: 15_000,
      env: { ...process.env, QUIRE_BRIDGE: join(scratch, "missing", "bridge.json"), QUIRE_RUN: "" },
    });
    if (result.status !== 0) return `exited with ${result.status ?? result.signal}: ${result.stderr}`;
    const replies = result.stdout.split("\n").filter(Boolean).map((line) => JSON.parse(line));
    const tools = replies.find((reply) => reply.id === 2)?.result?.tools;
    const call = replies.find((reply) => reply.id === 3)?.result;
    if (tools?.length !== 14) return `listed ${tools?.length ?? "no"} tools, expected 14`;
    if (!call?.isError || call.content?.[0]?.text !== closedApp) return `answered a call with no app running with ${JSON.stringify(call)}`;
    return undefined;
  } finally {
    rmSync(scratch, { recursive: true, force: true });
  }
}
