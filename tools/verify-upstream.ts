import { readdirSync, readFileSync, lstatSync } from "node:fs";
import { resolve, join, relative } from "node:path";
import { execFileSync } from "node:child_process";

const source = resolve(process.argv[2] ?? "../zed");
const vendor = resolve("vendor/zed");
const manifest = JSON.parse(readFileSync(join(vendor, "UPSTREAM.json"), "utf8"));
const revision = execFileSync("git", ["-C", source, "rev-parse", "HEAD"], { encoding: "utf8" }).trim();
if (revision !== manifest.revision) throw new Error("The upstream checkout is at a different revision. Use the pinned snapshot before comparing vendored files.");
const modified = new Set(manifest.modifications.flatMap((item: { path?: string }) => item.path ? [item.path] : []));
let checked = 0;
function verify(path: string) {
  const info = lstatSync(path);
  if (info.isSymbolicLink()) throw new Error(`Vendored source contains a symlink: ${path}. Materialize it before verification.`);
  if (info.isDirectory()) {
    for (const name of readdirSync(path)) verify(join(path, name));
    return;
  }
  const name = relative(vendor, path).replaceAll("\\", "/");
  if (name === "UPSTREAM.json" || modified.has(name)) return;
  if (!readFileSync(path).equals(readFileSync(join(source, name)))) throw new Error(`Undocumented upstream modification: ${name}. Review the change and update UPSTREAM.json.`);
  checked++;
}
verify(vendor);
console.log(`Verified ${checked} copied files against ${revision}; ${modified.size} documented fork modifications need diff review.`);
