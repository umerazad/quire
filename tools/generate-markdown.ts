import { copyFileSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { execFileSync } from "node:child_process";

const version = execFileSync("tree-sitter", ["--version"], { encoding: "utf8" }).trim();
if (version !== "tree-sitter 0.26.11") throw new Error(`Cannot reproduce the bundled grammar with ${version}. Use tree-sitter CLI 0.26.11 and rerun this tool.`);
const directory = mkdtempSync(join(tmpdir(), "quire-markdown-generate-"));
const root = resolve("vendor/quire-markdown/tree-sitter-markdown");
const env = { ...process.env };
for (const name of Object.keys(env)) if (name.startsWith("EXTENSION_") || name === "ALL_EXTENSIONS" || name === "NO_DEFAULT_EXTENSIONS") delete env[name];
try {
  execFileSync("tree-sitter", ["generate", "--abi", "14", "--output", directory, "grammar.js"], { cwd: root, env, stdio: "inherit" });
  for (const file of ["parser.c", "grammar.json", "node-types.json", "tree_sitter/parser.h"]) copyFileSync(join(directory, file), join(root, "src", file));
} finally {
  rmSync(directory, { recursive: true, force: true });
}
console.log("Regenerated the namespaced Markdown block parser with ABI 14.");
