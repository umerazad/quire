import { writeFileSync } from "node:fs";
import { pageTitle, frontmatterTags } from "../tests/reference/markdown/markdown";

const titles = [
  "plain", "raw  ", "'single quoted'", "'mismatched\"", "\"quoted میٹنگ 🙂\"", "\"trailing quote\"  ",
  '"\\ud800"', '"\\udc00"', '"\\ud800x\\udfff"', '"\\ud83d\\ude42"', '"\\u2028"',
  "null", "true", "false", "0", "-0", "1e400", "-1e400", "1e-400", "-1e-400", "1e21", "1e-7", "1e-6",
  "[]", "[null]", "[null,null]", "[[],[]]", '[1,null,[true,"x"]]', '["\\ud800",["\\udc00"]]',
  "{}", '{"toString":null}', '[{"toString":null}]', '{"toString":[]}', '{"valueOf":null}',
  '{"nested":{"toString":null}}', '{"__proto__":{"toString":null}}', '{"toString":1,"toString":2}',
  "01", "NaN", "undefined", "1n", "[1,]", '"\\q"', "'", '"', "'   '", "[".repeat(128) + '"deep"' + "]".repeat(128),
];
const headers = ["title: ", "title:", "title:\n", "title:\t", "title:\u00a0", "title:\ufeff", "title:\n \n", "Title: ", " title: ", "title : "];
const tagLines = [
  "tags: [one, two, #three]", "tag: one, two", "TAGS: ['one', \"two\", #three]", "tags:\n - one\n\t- #two\n  - 'three'\nnot-list: value",
  "tags: [one, one]\ntag: one\ntags:\n- two\n- two", "tags: [\"two words\",'three words']", " tags: ignored", "tags\t : a,b",
  "tags:\n-\u00a0ignored\n- later", "tags:\n - first\n\n - ignored", "tags:\n - first\n# comment\n - ignored", "tags: \"# spaced\"",
  "tags: [a,b]\u2028", "tags: [a,\u2028b]", "tags: [a,b]\r", "tags: [a,b]\r\r", "tags: [a,b]\u2028\u2029",
  "tags:\n - one\u2028two\n - ' # spaced '\n - ###three", "tag: []\ntags: \"\"\ntag: #", "tags: [[a]],b]\ntag: '[x]'",
];
const inputs: { name: string; path: string; text: string }[] = [];
const add = (name: string, body: string, ending = "\n", bom = "") => inputs.push({ name, path: "People/Fallback.MD", text: `${bom}---${ending}${body.replaceAll("\n", ending)}${ending}---${ending}` });
for (const [index, title] of titles.entries()) add(`title-${index}`, `title: ${title}`);
for (const [index, tags] of tagLines.entries()) add(`tags-${index}`, `title: \"Title\"\n${tags}`, index % 2 ? "\r\n" : "\n", index % 3 ? "" : "\ufeff");
for (const [index, body] of ["title:", "title:   ", "title:\n   ", "title:\n\nnext: word", "title: a\u2028tail", "title: a\rnext", "title: a\u2029tail", "title: first\ntitle: second", "---\ntitle: later\ntags: [after, empty]", "tags:\n - title: literal tag\n - end\ntag: final"].entries()) add(`special-${index}`, body);
for (const [index, text] of ["", "title: outside\ntags: [outside]", "---\n---\nbody", "---\ntitle: unfinished", " ---\ntitle: ignored\n---\n", "---\ntitle: EOF\n---", "\ufeff---\r\ntitle: BOM\r\ntag: اردو\r\n---", "---\ntitle: valid\n---\ntags: [outside]"].entries()) inputs.push({ name: `envelope-${index}`, path: index % 2 ? "Folder/name.md" : "Trailing/", text });
let state = 0x4317a9f;
const random = (length: number) => { state ^= state << 13; state ^= state >>> 17; state ^= state << 5; return (state >>> 0) % length; };
const pick = <T>(values: readonly T[]) => values[random(values.length)];
for (let index = 0; index < 640; index++) {
  const value = index % 5 ? pick(titles) : `${random(2) ? "-" : ""}${1 + random(999999)}.${random(100000)}e${random(700) - 350}`;
  const body = `${pick(headers)}${value}\n${pick(tagLines)}${random(3) ? "" : "\ntitle: repeated"}`;
  add(`seeded-${index}`, body, random(2) ? "\n" : "\r\n", random(3) ? "" : "\ufeff");
}
const vectors = inputs.map((input) => {
  const title = pageTitle(input.path, input.text);
  return { ...input, titleUtf16: Array.from({ length: title.length }, (_, index) => title.charCodeAt(index)), tags: frontmatterTags(input.text) };
});
writeFileSync("tests/fixtures/markdown/frontmatter-vectors.json", JSON.stringify(vectors, null, 2) + "\n");
console.log(`Generated ${vectors.length} title/tag vectors from the frozen Markdown module.`);
