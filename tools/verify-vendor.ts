import { readdirSync, lstatSync, readFileSync } from "node:fs";
import { resolve, join, relative, sep, isAbsolute } from "node:path";
import { execFileSync } from "node:child_process";

const root = resolve(process.argv.slice(2).find(argument => argument !== "--tracked") ?? "vendor/zed");
const repo = process.argv.includes("--tracked") ? execFileSync("git", ["rev-parse", "--show-toplevel"], { encoding: "utf8" }).trim() : undefined;
const tracked = repo ? new Set(execFileSync("git", ["-C", repo, "ls-files", "-z"], { encoding: "utf8" }).split("\0")) : undefined;
const manifest = JSON.parse(readFileSync(join(root, "UPSTREAM.json"), "utf8"));
for (const input of manifest.build_inputs ?? []) {
  const path = resolve(root, input);
  const name = relative(root, path);
  if (!name || name === ".." || name.startsWith(".." + sep) || isAbsolute(name)) throw new Error(`Invalid vendored build input: ${input}. Use a path inside the vendor directory.`);
  try {
    if (!lstatSync(path).isDirectory()) throw new Error("Not a directory");
  } catch {
    throw new Error(`Vendored build input is missing: ${input}. Copy it from the pinned snapshot before building.`);
  }
}
function verify(path: string) {
  const info = lstatSync(path);
  if (info.isSymbolicLink()) throw new Error(`Vendored source contains a symlink: ${path}. Copy the target into this repository before building.`);
  if (info.isDirectory()) for (const name of readdirSync(path)) verify(join(path, name));
  else if (repo && !tracked?.has(relative(repo, path).replaceAll("\\", "/"))) throw new Error(`Vendored tree contains an untracked file: ${path}. Remove generated files or track intended source files, then retry.`);
}
verify(root);
console.log("Vendored source contains no symlinks.");
