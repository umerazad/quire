import { writeFileSync } from "node:fs";
import { parseMarkdown } from "../tests/reference/markdown/markdown";

const urls = ["http://example.test", "https://example.test/path", "www.example.test", "http://localhost", "www.local", "HTTPS://example.test/path", "http://sub_name.example.test/a", "http://sub.example_name.test/a", "https://a.b:123/a", "https://a.b/a?b!", "https://a.b/(x)))", "https://a.b/a&amp;", "https://a.b/a&#123;", "https://a.b/a&#xabc;", "https://a.b/a&#XABC;", "https://a.b/a??!.,:*_~", "https://a.b/[#tag]", "https://a.b/unclosed[#tag", "https://a.b/x]rest", "https://a.b/a\ufeff#tag", "https://a.b/a\u0085#tag", "https://a.b/a\u2028#tag", "https://a.b/a\u2029#tag", "https://a.b/a\r\n#tag", "https://a.b/مرحبا🙂", "a.b+tag@example.test", "a@b.c_", "a@b.c-", "a@b.c..", "mailto:a.b+tag@example.test", "xmpp:a@example.test/resource@name.1", "xmpp:a@example.test/resource_bad", "mailto:invalid", "a".repeat(100) + "@example.test", "a".repeat(101) + "@example.test"];
urls.push("x:a", "x_:a", "x".repeat(64) + ":/a.b/[#tag]", "ab:one<two", "a@" + "a".repeat(64) + ".b", "a@b." + "x".repeat(64), "https://example.test/a&#xA;", "xmpp:a@b.c/r..._rest", "https://a.b/[x[y]z]");
const prefixes = ["", "prefix ", "prefix α", "prefix a", "🙂 ", "[", "prefix [", "("];
const inputs = urls.flatMap((url, index) => prefixes.map((prefix, part) => ({ name: `form-${index}-${part}`, text: prefix + url + " suffix", start: prefix.length, inLink: prefix.endsWith("[") })));
let state = 0x1834ab7;
const random = (length: number) => { state ^= state << 13; state ^= state >>> 17; state ^= state << 5; return (state >>> 0) % length; };
const schemes = ["http://", "https://", "www.", "mailto:", "xmpp:", "", "HTTPS://"];
const domains = ["example.test", "a.b", "sub_name.example.test", "sub.example_name.test", "a..b", "localhost", "user@a.b", "user@a.b..", "user@a.b_", "a.b:9000"];
const paths = ["", "/", "/a", "/[#tag]", "/[#tag", "/#tag", "/x]rest", "/(a)))", "/x&amp;", "/x&#12;", "/x;", "/x_~", "/x??", "/x\ufeff#tag", "/x\u0085#tag", "/x\u2028#tag", "/a🙂ب"];
for (let index = 0; index < 512; index++) {
  const prefix = prefixes[random(prefixes.length)];
  inputs.push({ name: `seeded-${index}`, text: prefix + schemes[random(schemes.length)] + domains[random(domains.length)] + paths[random(paths.length)] + " suffix", start: prefix.length, inLink: prefix.endsWith("[") });
}
const result = inputs.map(input => {
  let end: number | null = null;
  parseMarkdown(input.text).iterate({ enter(node) {
    if (node.name === "URL" && node.from === input.start) end = node.to;
  } });
  const angleSource = "<" + input.text.slice(input.start, -" suffix".length) + ">";
  let angle = false;
  parseMarkdown("prefix " + angleSource).iterate({ enter(node) {
    if (node.name === "Autolink" && node.from === 7 && node.to === 7 + angleSource.length) angle = true;
  } });
  return { ...input, end, angleSource, angle };
});
writeFileSync("tests/fixtures/markdown/bare-url-vectors.json", "[\n" + result.map(vector => JSON.stringify(vector)).join(",\n") + "\n]\n");
console.log(`Generated ${result.length} bare URL scanner vectors from the frozen Markdown parser.`);
