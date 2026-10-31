// Drives the compiled helper: measures spawn-to-first-response, then warm per-request latency.
// Usage: bun run-helper.ts <helper-binary> <out-dir> [repeats=20] [corpus-dir=corpus]
import { readdirSync } from "node:fs";

const [bin, outDir, rep, corpusDir = "corpus"] = process.argv.slice(2);
const repeats = Number(rep ?? 20);
const here = new URL(".", import.meta.url).pathname;
const parsers: Record<string, string> = { js: "babel", javascript: "babel", jsx: "babel", ts: "typescript", typescript: "typescript", tsx: "typescript", json: "json", css: "css", html: "html", xml: "html", yaml: "yaml", yml: "yaml", markdown: "markdown", md: "markdown" };
await Bun.$`mkdir -p ${outDir}`;

const t0 = performance.now();
const proc = Bun.spawn([bin], { stdin: "pipe", stdout: "pipe" });
const reader = proc.stdout.getReader();
const decoder = new TextDecoder();
let buf = "";
async function request(id: number, parser: string, text: string) {
  proc.stdin.write(JSON.stringify({ id, parser, text }) + "\n");
  proc.stdin.flush();
  while (!buf.includes("\n")) {
    const { value, done } = await reader.read();
    if (done) throw new Error("helper exited");
    buf += decoder.decode(value, { stream: true });
  }
  const nl = buf.indexOf("\n");
  const line = buf.slice(0, nl);
  buf = buf.slice(nl + 1);
  return JSON.parse(line);
}
const first = await request(0, "babel", "const x={a:1}");
const coldMs = performance.now() - t0;

const timings: Record<string, [number, number]> = {};
let id = 1;
for (const file of readdirSync(`${here}${corpusDir}`).sort()) {
  const lang = file.split(".").pop()!;
  const input = await Bun.file(`${here}${corpusDir}/${file}`).text();
  let t = performance.now();
  const res = await request(id++, parsers[lang], input);
  const firstMs = performance.now() - t;
  t = performance.now();
  for (let i = 0; i < repeats; i++) await request(id++, parsers[lang], input);
  const warmMs = (performance.now() - t) / repeats;
  timings[file] = [+firstMs.toFixed(2), +warmMs.toFixed(2)];
  await Bun.write(`${outDir}/${file}`, res.error ? `ERROR: ${res.error.split("\n")[0]}\n` : res.text);
}
proc.stdin.end();
await proc.exited;
console.log(JSON.stringify({ tool: bin.split("/").pop(), first_result_ok: first.text === "const x = { a: 1 };\n", spawn_to_first_format_ms: +coldMs.toFixed(1), timings_first_warm_ms: timings }));
