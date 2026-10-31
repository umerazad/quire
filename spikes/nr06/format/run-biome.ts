// Formats the corpus with the Biome CLI (one process per snippet, as an external helper would).
import { readdirSync } from "node:fs";

const here = new URL(".", import.meta.url).pathname;
const biome = `${here}node_modules/.bin/biome`;
const ext: Record<string, string> = { js: "jsx", javascript: "jsx", jsx: "jsx", ts: "ts", typescript: "ts", tsx: "tsx", json: "json", css: "css", html: "html", xml: "html", yaml: "yaml", yml: "yaml", markdown: "md", md: "md" };
await Bun.$`mkdir -p ${here}out-biome`;
const timings: Record<string, number> = {};
for (const file of readdirSync(`${here}corpus`).sort()) {
  const lang = file.split(".").pop()!;
  const input = await Bun.file(`${here}corpus/${file}`).text();
  let out: string;
  const t = performance.now();
  if (!ext[lang]) out = "ERROR: Biome has no formatter for this language\n";
  else {
    const p = Bun.spawnSync([biome, "format", `--config-path=${here}biome`, `--stdin-file-path=snippet.${ext[lang]}`], { stdin: Buffer.from(input) });
    out = p.exitCode === 0 ? p.stdout.toString() : `ERROR: ${p.stderr.toString().split("\n").find((l) => l.trim()) ?? "failed"}\n`;
  }
  timings[file] = +(performance.now() - t).toFixed(1);
  await Bun.write(`${here}out-biome/${file}`, out);
}
console.log(JSON.stringify({ tool: "biome-2.6.4-cli-per-call", timings_ms: timings }));
