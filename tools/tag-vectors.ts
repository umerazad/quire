import { writeFileSync } from "node:fs";
import { editorText, parseMarkdown, tagCompletionRange, tagExcludedRanges, tags } from "../tests/reference/markdown/markdown";

const forms = ["#tag", "#", "#_", "#123", "#work/project", "#one/two/three", "#one//two", "#one/", "#-bad", "#/bad", "#one-", "#_/-", "#مرحبا", "#اُردُو", "#عربي/موضوع", "#e\u0301", "#é", "#中文/项目", "#𐐀/𐐁", "#Ⅳ/²", "🙂#tag", "🙂 #tag", "\u0085#tag", "\ufeff#tag", "\u2028#tag", "(#tag)", "[#tag]", "{#tag}", "\"#tag\"", "'#tag'", "*#tag*", "~#tag~", "a#tag", "##tag", "#a#b", "#a,#b", "#a #b", "#a\n#b", "#a\r\n#b", "\\#escaped", "[[#page]] #outside", "((#block)) #outside", "![[#embed]]", "{{pdf: #file.pdf}}", "{{embed: [[#page]]}}"];
const wrappers = [
  (s: string) => s,
  (s: string) => `- ${s}\n  - #child\n`,
  (s: string) => `> ${s}\n`,
  (s: string) => `\`${s}\` #outside\n`,
  (s: string) => `\`\`\`md\n${s}\n\`\`\`\n#outside\n`,
  (s: string) => `    ${s}\n\n#outside\n`,
  (s: string) => `[${s}](https://example.test/#url) #outside`,
  (s: string) => `![${s}](image.png) #outside`,
  (s: string) => `<https://example.test/${s}> #outside`,
  (s: string) => `<div>${s}</div>\n\n#outside`,
  (s: string) => `text <span title="${s}">#inside</span> #outside`,
  (s: string) => `[name]: https://example.test/${s}\n\n#outside`,
  (s: string) => `---\ntitle: ${s}\n---\n\n#outside`,
  (s: string) => `| Value |\n| --- |\n| ${s} |\n`,
];
const inputs = forms.map((source, index) => ({ name: `form-${index}`, source }));
for (const [index, source] of ["`#hidden\n===\ncontinued`\n#visible", "#before `#code` #after", "#before [label](#destination) #after", "[label #hidden][ref]\n\n[ref]: url\n#visible", "[missing #visible][undefined]", "<!-- #hidden --> #visible", "<div>\n#hidden\n</div>\n\n#visible", "#tag\n", "#tag\r", "#tag\u2028", "#tag\u2029", "#a/ب/🙂", "#a\u200dب", "#a\u00a0#b", "\u0085#bad\ufeff#good", "{{pdf: before #hidden}} #visible", "[[cross `code]]` #visible", "#a".repeat(32)].entries()) inputs.push({ name: `boundary-${index}`, source });
let state = 0x19fda37;
const random = (length: number) => { state ^= state << 13; state ^= state >>> 17; state ^= state << 5; return (state >>> 0) % length; };
for (let index = 0; index < 512; index++) {
  let source = Array.from({ length: 1 + random(4) }, () => wrappers[random(wrappers.length)](forms[random(forms.length)])).join(random(2) ? "\n" : " ");
  if (random(3) === 0) source = source.replaceAll("\n", "\r\n");
  inputs.push({ name: `seeded-${index}`, source });
}
for (const [index, source] of ["text <!-- #comment --> #outside","<?pi #processing ?> #outside","text <?pi #processing ?> #outside","<![CDATA[ #hidden ]]>\n\n#outside","<!DOCTYPE #hidden>\n\n#outside","https://example.test/[#tag] #outside","www.example.test/[#tag] #outside","https://example.test/`code #one` now `#two`","`https://example.test/path`[#tag]","[https://example.test/path] [#outside]","<a_b:[#tag]> #outside","<aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa:[#tag]> #outside","<https://example.test/<[#tag]> #outside","https://example.test/`[[Page]]` [[Outside]]"].entries()) inputs.push({ name: `compat-${index}`, source });
inputs.push({ name: "doctype-code", source: "<!DOCTYPE html>\n\n`#hidden` #visible\n" });
inputs.push({ name: "doctype-multiline", source: "<!DOCTYPE html\n #hidden>\n\n #visible" });
inputs.push({ name: "doctype-quote", source: "> <!DOCTYPE html>\n>\n> #visible\n" });
inputs.push({ name: "url-quote", source: "> مرحبا🙂\n> https://example.test/`code #one` now `#two`\n" });
inputs.push({ name: "url-list", source: "- مرحبا🙂\n  https://example.test/`code #one` now `#two`\n" });
inputs.push({ name: "invalid-angle-code", source: "<ab:body `prefix #hidden`> #outside" });
function vector(text: string, exhaustive: boolean) {
  const tree = parseMarkdown(text);
  const codeRanges: [number, number][] = [];
  tree.iterate({ enter(node) {
    if (["FencedCode", "CodeBlock", "InlineCode"].includes(node.name)) {
      codeRanges.push([node.from, node.to]);
      return false;
    }
  } });
  const boundaries = [0];
  for (const character of text) boundaries.push(boundaries.at(-1)! + character.length);
  const positions = boundaries.filter((position, index) => exhaustive || index % Math.max(1, Math.ceil(boundaries.length / 24)) === 0 || text.slice(Math.max(0, position - 10), position).includes("#") || position === text.length);
  const completions = positions.map(position => {
    const range = tagCompletionRange(text, position, tree);
    return [position, range ? [range.from, range.to] : null];
  });
  return { text, codeRanges, syntaxRanges: tagExcludedRanges(text, tree, []), tags: tags(text, tree), completions };
}
const result = inputs.map(({ name, source }) => ({ name, raw: vector(source, !name.startsWith("seeded")), editor: vector(editorText(source), !name.startsWith("seeded")) }));
writeFileSync("tests/fixtures/markdown/tag-vectors.json", "[\n" + result.map(vector => JSON.stringify(vector)).join(",\n") + "\n]\n");
console.log(`Generated ${result.length} raw/editor tag and completion vectors from the frozen Markdown module.`);
