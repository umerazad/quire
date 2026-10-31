// Long-lived formatter helper: one JSON request per stdin line, one JSON response per stdout line.
// Request: {"id":1,"parser":"babel","text":"..."}; response: {"id":1,"text":"..."} or {"id":1,"error":"..."}.
// Options match rt 0.9.57 src/editor/code-tools.ts.
import { format } from "prettier/standalone";
import * as babel from "prettier/plugins/babel";
import * as estree from "prettier/plugins/estree";
import * as typescript from "prettier/plugins/typescript";
import * as postcss from "prettier/plugins/postcss";
import * as html from "prettier/plugins/html";
import * as markdown from "prettier/plugins/markdown";
import * as yaml from "prettier/plugins/yaml";

const plugins = [babel, estree, typescript, postcss, html, markdown, yaml];
const decoder = new TextDecoder();
let buffer = "";
for await (const chunk of Bun.stdin.stream()) {
  buffer += decoder.decode(chunk, { stream: true });
  let nl: number;
  while ((nl = buffer.indexOf("\n")) >= 0) {
    const line = buffer.slice(0, nl);
    buffer = buffer.slice(nl + 1);
    if (!line.trim()) continue;
    const req = JSON.parse(line);
    try {
      const text = await format(req.text, { parser: req.parser, plugins, tabWidth: 2 });
      process.stdout.write(JSON.stringify({ id: req.id, text }) + "\n");
    } catch (e) {
      process.stdout.write(JSON.stringify({ id: req.id, error: String(e) }) + "\n");
    }
  }
}
