import { writeFileSync } from "node:fs";
import { editorText, parseMarkdown, references } from "../tests/reference/markdown/markdown";

const forms = ["[[Page]]", "((block))", "![[Embed]]", "{{embed: ((block))}}", "{{embed: [[Page]]}}", "{{[[embed]]: [[Page#^id|label]]}}", "[[Page#^block]]", "[[Page|Label|ignored]]", "((block|Label|extra))", "[[|label]]", "[[page|]]", "[[ ]]", "(( ))", "{{pdf: docs/file.pdf}}", "{{[[pdf]]: docs/file.pdf}}", "{{pdf: [[Inner]]|label}}", "{{pdf:   }}", "{{pdf:\n file.pdf\n}}", "{{embed:\n ((id))\n}}", "{{embed:\ufeff[[میٹنگ🙂]]\u2028}}", "{{pdf:\ufefffile.pdf\u0085}}", "[[عربی🙂|اُردُو]]", "[[e\u0301]]", "{{PDF: ignored}}", "{{[[PDF]]: file.pdf}}", "[[unfinished", "((unfinished", "[[nested [[Page]] tail]]", "{{embed: [[missing}}", "[[a\rb]]", "[[a\nb]]", "{{pdf: a\nb}}", "\\[[escaped]]", "![[media.pdf#page=3|label]]", "{{[[embed]]: ((id))}}"];
const wrap = [
  (value: string) => value,
  (value: string) => `🙂 prefix ${value} suffix\n`,
  (value: string) => `- Parent\n  - ${value}\n`,
  (value: string) => `> ${value}\n`,
  (value: string) => `\`${value}\`\n`,
  (value: string) => `\`\`${value}\`\`\n`,
  (value: string) => `\`\`\`md\n${value}\n\`\`\`\n`,
  (value: string) => `    ${value}\n`,
  (value: string) => `---\ntitle: ${value}\n---\n\n[[Outside]]\n`,
  (value: string) => `<div>${value}</div>\n`,
  (value: string) => `[label ${value}](https://example.test)\n`,
  (value: string) => `| Value |\n| --- |\n| ${value} |\n`,
];
const inputs = forms.map((source, index) => ({ name: `form-${index}`, source }));
for (const [index, source] of ["[[before `code]]` after", "`code [[inside` after]]", "`[[code]]\n===\ncontinued`\n", "`[[code]]\n\ncontinued`\n", "- parent\n  ```\n  [[code]]\n  ```\n  - [[visible]]\n", "---\n---\ntitle: [[metadata]]\n---\n[[visible]]\n", "{{pdf: [[inside]]}} [[outside]]", "{{pdf: before `[[inside]]` after}}", "[[first]][[second]]((third))![[fourth]]", "\u0085{{embed: \u0085[[x]]}}\ufeff", "[[".repeat(24) + "page]]", "No reference text\n".repeat(80)].entries()) inputs.push({ name: `boundary-${index}`, source });
let state = 0x791cab3;
const random = (length: number) => { state ^= state << 13; state ^= state >>> 17; state ^= state << 5; return (state >>> 0) % length; };
for (let index = 0; index < 512; index++) {
  const parts = Array.from({ length: 1 + random(5) }, () => wrap[random(wrap.length)](forms[random(forms.length)]));
  let source = parts.join(random(2) ? "\n" : " ");
  if (random(3) === 0) source = source.replaceAll("\n", "\r\n");
  inputs.push({ name: `seeded-${index}`, source });
}
function vector(text: string) {
  const codeRanges: [number, number][] = [];
  const tree = parseMarkdown(text);
  tree.iterate({ enter(node) {
    if (["FencedCode", "CodeBlock", "InlineCode"].includes(node.name)) {
      codeRanges.push([node.from, node.to]);
      return false;
    }
  } });
  return { text, codeRanges, references: references(text, tree) };
}
const vectors = inputs.map(({ name, source }) => ({ name, raw: vector(source), editor: vector(editorText(source)) }));
writeFileSync("tests/fixtures/markdown/reference-vectors.json", JSON.stringify(vectors, null, 2) + "\n");
console.log(`Generated ${vectors.length} raw/editor reference vectors from the frozen Markdown module.`);
