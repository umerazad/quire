// Reference outputs: the exact call rt 0.9.57 makes in src/editor/code-tools.ts.
import { format } from "prettier/standalone";
import * as babel from "prettier/plugins/babel";
import * as estree from "prettier/plugins/estree";
import * as typescript from "prettier/plugins/typescript";
import * as postcss from "prettier/plugins/postcss";
import * as html from "prettier/plugins/html";
import * as markdown from "prettier/plugins/markdown";
import * as yaml from "prettier/plugins/yaml";
import { cases } from "./corpus";

const aliases: Record<string, string> = { js: "javascript", jsx: "javascript", ts: "typescript", tsx: "typescript", html: "xml", yml: "yaml" };
const parsers: Record<string, string> = { javascript: "babel", typescript: "typescript", json: "json", css: "css", xml: "html", yaml: "yaml", markdown: "markdown", md: "markdown" };
const plugins = [babel, estree, typescript, postcss, html, markdown, yaml];
const out = new URL("./expected/prettier/", import.meta.url).pathname;
await Bun.$`mkdir -p ${out}`;
const timings: Record<string, number> = {};
for (const c of cases) {
  const parser = parsers[aliases[c.lang] ?? c.lang];
  let text: string;
  const t = performance.now();
  try {
    text = await format(c.input, { parser, plugins, tabWidth: 2 });
  } catch (e) {
    text = `ERROR: ${String(e).split("\n")[0]}\n`;
  }
  timings[c.name] = +(performance.now() - t).toFixed(2);
  await Bun.write(`${out}${c.name}.${c.lang}`, text);
}
console.log(JSON.stringify({ tool: "prettier-3.9.10-bun-first-pass", timings_ms: timings }));
