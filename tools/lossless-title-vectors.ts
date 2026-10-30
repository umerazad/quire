import { writeFileSync } from "node:fs";
import { editorText, pageTitle } from "../tests/reference/markdown/markdown";
const paths = ["People/Note.md", "Folder/\ud800.MD", "\udfff.md", "Folder/🙂.md", "Folder/", "\ud800/Leaf.md"];
const values = ["\ud800", "\udc00", "\ud800x\udfff", "🙂", "\ufffd", '"\ud800"', "'\ud800'", '"\\\ud800"', '"\\\\\ud800"', '["\ud800",null,"\udfff"]', '{"\ud800":1}', '{"toString":"\ud800"}', '"unterminated\ud800', "\ud800 tail  ", " \t", "", "\ud800\u2028after", "[null,\ud800]"];
const cases = paths.flatMap(path => values.flatMap(value => [
  "", "body \ud800", `---\ntitle: ${value}\n---\nbody`, `\ufeff---\r\ntitle: ${value}\r\n---\r\n\ud800`, `---\n\ud800\ntitle: ${value}\n---\n`, `---\ntitle: ${value}\n---`, `---\ntitle: ${value}\nmissing`, `---\ntitle\ud800: ignored\ntitle: ${value}\n---\n`, `---\rtitle: ${value}\r---\rbody`, `---\r\ntitle: ${value}\r---\nbody`,
].map(text => {
  const title = pageTitle(path, text);
  const editorTitle = pageTitle(path, editorText(text));
  return {path, text, titleUtf16: Array.from({length: title.length}, (_, index) => title.charCodeAt(index)), editorTitleUtf16: Array.from({length: editorTitle.length}, (_, index) => editorTitle.charCodeAt(index))};
})));
writeFileSync("tests/fixtures/markdown/lossless-title-vectors.json", JSON.stringify(cases, null, 2) + "\n");
console.log(`Generated ${cases.length} lossless title vectors.`);
