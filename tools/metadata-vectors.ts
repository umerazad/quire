import { writeFileSync } from "node:fs";
import { frontmatterLength, pageName, relativeVaultPath, safePagePath } from "../tests/reference/markdown/markdown";

const titles = ["Plan", " میٹنگ 🙂 ", "e\u0301", "", "\u0085", "\ufeff Plan\ufeff", "\u200bPlan", ".", "..", ".hidden", "trailing.", "a/b", "a\\b", "a:b", "a\0b", "a\u007fb", "a\u009fb", "CON", "con.txt", "aux", "PRN.md", "NUL", "COM0", "com9.foo", "LPT1", "lpt9", "com10", "console", "CONsole", "a".repeat(150), "a".repeat(151), "🙂".repeat(75), "🙂".repeat(76)];
const links = ["A.md", "../A.md", "../../A.md", "/A.md", "%2FA.md", "./A.md#^id", "#Heading", "", "a%23b.md#ignored", "%E2%82%AC.md", "%F0%9F%99%82.md", "%", "%G0", "%0", "%FF", "%C0%AF", "%ED%A0%80", "%E0%A4%A", "a%2Fb.md", "%00", "C%3A/a.md", "a%5Cb.md", ".././../x", "/../../x", "///a//./b/../c.md"];
const texts = ["", "---\n---\n", "---\n\n---", "\ufeff---\r\ntitle: میٹنگ 🙂\r\n---\r\nbody", "---\ntitle: A\n---\nbody", "---\nfoo\n--- extra\n---\nbody", "---\r\nfoo\r\n---", "---\nfoo\n---\r", "---\nfoo\n---\r\n", "---\nmissing", " ---\nfoo\n---\n", "\ufeff\ufeff---\nfoo\n---\n", "---\nfoo\n---x\nbody", "---\n🙂\u2028---\n", "---\rfoo\r---\r", "---\n---\nbody\n---\n"];
const froms = [undefined, "Plan.md", "People/Plan.md", "Projects/Work/A.md", "", "People//A.md"];
let state = 0x173ba24;
const random = (length: number) => {
  state ^= state << 13;
  state ^= state >>> 17;
  state ^= state << 5;
  return (state >>> 0) % length;
};
const chars = ["a", "Z", ".", " ", "\t", "\n", "\u0085", "\u00a0", "\ufeff", "é", "e\u0301", "م", "ی", "🙂", "<", "*", "?", "/", "\\", "#", "%", "^", "\u2028"];
const cases = Array.from({ length: 640 }, (_, index) => {
  const title = index < titles.length ? titles[index] : Array.from({ length: 1 + random(18) }, () => chars[random(chars.length)]).join("");
  const target = index < links.length ? links[index] : links[random(links.length)] + (random(3) ? "" : "#🙂");
  const from = froms[index % froms.length];
  const markdown = index < texts.length ? texts[index] : `${random(2) ? "\ufeff" : ""}---${random(2) ? "\r\n" : "\n"}${title}\n${random(2) ? "---\n" : "---"}body\n${random(2) ? "---\r\n" : ""}`;
  const pagePath = `${index % 2 ? "People/" : ""}${title}${[".md", ".MD", ".Md", ".mD", ".markdown", "md", ""][index % 7]}`;
  let path: string | null = null, error: string | null = null;
  try { path = safePagePath(title); } catch (caught) { error = (caught as Error).message; }
  return { title, path, error, from: from ?? null, target, relative: relativeVaultPath(from, target) ?? null, markdown, frontmatterUtf16: frontmatterLength(markdown), pagePath, pageName: pageName(pagePath) };
});
writeFileSync("tests/fixtures/markdown/metadata-vectors.json", JSON.stringify(cases, null, 2) + "\n");
console.log(`Generated ${cases.length} metadata vectors from the frozen Markdown module.`);
