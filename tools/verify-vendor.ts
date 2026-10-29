import { readdirSync, lstatSync } from "node:fs";
import { resolve, join } from "node:path";
import { relative } from "node:path";
import { execFileSync } from "node:child_process";

const root = resolve(process.argv.slice(2).find(argument => argument !== "--tracked") ?? "vendor/zed");
const repo = process.argv.includes("--tracked") ? execFileSync("git", ["rev-parse", "--show-toplevel"], { encoding: "utf8" }).trim() : undefined;
const tracked = repo ? new Set(execFileSync("git", ["-C", repo, "ls-files", "-z"], { encoding: "utf8" }).split("\0")) : undefined;
function verify(path: string) {
  const info = lstatSync(path);
  if (info.isSymbolicLink()) throw new Error(`Vendored source contains a symlink: ${path}. Copy the target into this repository before building.`);
  if (info.isDirectory()) for (const name of readdirSync(path)) verify(join(path, name));
  else if (repo && !tracked?.has(relative(repo, path).replaceAll("\\", "/"))) throw new Error(`Vendored tree contains an untracked file: ${path}. Remove generated files or track intended source files, then retry.`);
}
verify(root);
console.log("Vendored source contains no symlinks.");
