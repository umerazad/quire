import { readdirSync, lstatSync } from "node:fs";
import { resolve, join } from "node:path";

const root = resolve(process.argv[2] ?? "vendor/zed");
function verify(path: string) {
  const info = lstatSync(path);
  if (info.isSymbolicLink()) throw new Error(`Vendored source contains a symlink: ${path}. Copy the target into this repository before building.`);
  if (info.isDirectory()) for (const name of readdirSync(path)) verify(join(path, name));
}
verify(root);
console.log("Vendored source contains no symlinks.");
