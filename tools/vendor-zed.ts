import { cpSync, existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";
import { execFileSync } from "node:child_process";

const source = resolve(process.argv[2] ?? "../zed");
const root = Bun.TOML.parse(readFileSync(`${source}/Cargo.toml`, "utf8"));
const dependencies = root.workspace.dependencies;
const selected = new Map<string, any>();
const required = new Set<string>();
function visit(name: string) {
  if (selected.has(name)) return;
  const dependency = dependencies[name];
  if (!dependency?.path) return;
  const manifest = Bun.TOML.parse(readFileSync(`${source}/${dependency.path}/Cargo.toml`, "utf8"));
  selected.set(name, { path: dependency.path, manifest });
  const groups = [manifest.dependencies, manifest["dev-dependencies"], manifest["build-dependencies"],
    ...Object.values(manifest.target ?? {}).flatMap((target: any) => [target.dependencies, target["dev-dependencies"], target["build-dependencies"]])];
  for (const group of groups) for (const [key, value] of Object.entries(group ?? {})) {
    if ((value as any).workspace) { required.add(key); visit(key); }
    else if ((value as any).path) throw new Error(`Review direct path dependency ${name}/${key} before copying.`);
  }
}
visit("gpui_platform");
const destination = "vendor/zed";
if (existsSync(destination)) throw new Error("vendor/zed already exists. Review its modifications before replacing the snapshot.");
mkdirSync(destination, { recursive: true });
for (const { path } of selected.values()) cpSync(`${source}/${path}`, `${destination}/${path}`, { recursive: true });
cpSync(`${source}/assets/fonts`, `${destination}/assets/fonts`, { recursive: true });
for (const license of ["LICENSE-APACHE", "LICENSE-GPL", "LICENSE-AGPL", "LICENSE-MIT"]) if (existsSync(`${source}/${license}`)) cpSync(`${source}/${license}`, `${destination}/${license}`);
function toml(value: any): string {
  if (value === null || value === undefined) throw new Error("Unsupported TOML value");
  if (Array.isArray(value)) return `[${value.map(toml).join(", ")}]`;
  if (typeof value === "object") return `{ ${Object.entries(value).map(([k,v]) => `${JSON.stringify(k)} = ${toml(v)}`).join(", ")} }`;
  return JSON.stringify(value);
}
const workspaceDependencies = Object.fromEntries([...required].sort().map(name => {
  const value = dependencies[name];
  return [name, value?.path ? { ...value, path: `${destination}/${value.path}` } : value];
}));
const patches = Object.fromEntries(Object.entries(root.patch?.["crates-io"] ?? {}).filter(([name, value]: any) => required.has(name) && !value.path));
const members = ["crates/quire", ...[...selected.values()].map(v => `${destination}/${v.path}`)];
let output = `[workspace]\nresolver = "3"\nmembers = ${toml(members)}\ndefault-members = ["crates/quire"]\n\n[workspace.package]\n`;
for (const [k,v] of Object.entries(root.workspace.package)) output += `${k} = ${toml(v)}\n`;
output += "\n[workspace.dependencies]\n";
for (const [k,v] of Object.entries(workspaceDependencies)) output += `${JSON.stringify(k)} = ${toml(v)}\n`;
output += "\n[workspace.lints]\n";
for (const [k,v] of Object.entries(root.workspace.lints ?? {})) output += `${k} = ${toml(v)}\n`;
if (Object.keys(patches).length) {
  output += "\n[patch.crates-io]\n";
  for (const [k,v] of Object.entries(patches)) output += `${JSON.stringify(k)} = ${toml(v)}\n`;
}
writeFileSync("Cargo.toml", output);
writeFileSync(`${destination}/UPSTREAM.json`, JSON.stringify({ repository: "https://github.com/zed-industries/zed", revision: execFileSync("git", ["-C", source, "rev-parse", "HEAD"], { encoding: "utf8" }).trim(), crates: [...selected.keys()].sort(), modifications: [] }, null, 2) + "\n");
console.log(`Copied ${selected.size} crates into ${destination}.`);
