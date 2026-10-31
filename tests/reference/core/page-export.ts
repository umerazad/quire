import { references, relativeVaultPath, type Reference } from "./export-markdown";
import { attachmentKind } from "./attachments";
import type { Notebook } from "./export-notebook";

const idMarker = /[ \t]+\^[\w-]+[ \t]*$/gm,
  createdStamp = /[ \t]*➕[ \t]*(?:\[\[[^\]\n]+\]\]|\d{4}-\d{2}-\d{2})/gu,
  fileTarget = /\.(?!md$)[a-z0-9]{2,5}$/i;
// Local attachments are written as quire-vault: links holding their path in
// the notebook; the desktop side turns them into file paths it has checked.
export const vaultScheme = "quire-vault:";
const vaultLink = (fromPath: string, target: string) => {
  const relative = relativeVaultPath(fromPath, target.split(/[?#]/)[0]);
  return relative && vaultScheme + encodeURIComponent(relative);
};
const markdownImage = /!\[([^\]\n]*)\]\(<?([^)>\s]+)>?(\s+"[^"]*")?\)/g;
const localImages = (text: string, fromPath: string) =>
  text.replace(markdownImage, (whole, alt: string, source: string) => {
    if (/^[a-z][a-z0-9+.-]*:/i.test(source) || source.startsWith("//"))
      return whole;
    const link = vaultLink(fromPath, decodeURI(source));
    return link ? `![${alt}](${link})` : whole;
  });
const clean = (text: string) =>
  text.replace(idMarker, "").replace(createdStamp, "");
// A referenced block and its children as Markdown, with the children
// re-indented to sit under the column the embed was written at.
function blockMarkdown(notebook: Notebook, target: string, column: number) {
  const found = notebook.resolve(target);
  if ("error" in found) return undefined;
  const text = notebook.text(found.path),
    { block } = found,
    lineStart = text.lastIndexOf("\n", block.from - 1) + 1,
    indent = block.from - lineStart,
    [first, ...rest] = text
      .slice(block.contentFrom, block.subtreeTo)
      .split("\n");
  return {
    path: found.path,
    markdown: [
      first,
      ...rest.map((line) =>
        line.trim() ? " ".repeat(column) + line.slice(indent) : line,
      ),
    ].join("\n"),
  };
}
function replacement(
  notebook: Notebook,
  text: string,
  ref: Reference,
  depth: number,
  fromPath: string,
) {
  const original = text.slice(ref.from, ref.to);
  if (ref.kind === "page") return ref.label ?? ref.target;
  if (ref.kind === "block") {
    const found = notebook.resolve(ref.target);
    return (
      ref.label ??
      ("error" in found
        ? original
        : exportText(
            notebook,
            clean(found.block.text).trim(),
            depth + 1,
            found.path,
          ))
    );
  }
  if (ref.media || attachmentKind(ref.target)) {
    const link = vaultLink(fromPath, ref.target);
    return link
      ? `![${ref.label ?? ref.target.split("/").at(-1)}](${link})`
      : original;
  }
  if (fileTarget.test(ref.target)) return original;
  const lineStart = text.lastIndexOf("\n", ref.from - 1) + 1,
    marker = /^[ \t]*(?:(?:[-+*]|\d+[.)])[ \t]+)?/.exec(
      text.slice(lineStart, ref.from),
    )![0],
    column = marker.length ? text.slice(lineStart).search(/\S/) : 0;
  const block = blockMarkdown(notebook, ref.target, column);
  if (block) return exportText(notebook, block.markdown, depth + 1, block.path);
  const page = notebook.findPage(ref.target);
  if (!page || depth >= maxDepth) return ref.label ?? ref.target;
  const body = exportText(
    notebook,
    notebook.text(page),
    depth + 1,
    page,
  ).trimEnd();
  // Inside a list item, the page becomes an item titled with its name and
  // its content nested below.
  return marker.length
    ? [
        notebook.title(page),
        ...body
          .split("\n")
          .map((line) => (line.trim() ? " ".repeat(column + 2) + line : line)),
      ].join("\n")
    : body;
}
// A page as standalone Markdown for a document outside the notebook: links
// read as their text, block references and embeds become what they point
// to, and block IDs and created-date stamps are dropped.
export const exportMarkdown = (notebook: Notebook, path: string) =>
  exportText(notebook, notebook.text(path), 0, path);
// Embeds and references nest at most this deep, which also stops cycles.
const maxDepth = 3;
function exportText(
  notebook: Notebook,
  text: string,
  depth: number,
  fromPath: string,
): string {
  if (depth > maxDepth) return clean(text);
  const refs = references(text).sort((a, b) => a.from - b.from);
  let result = "",
    last = 0;
  for (const ref of refs) {
    if (ref.from < last) continue;
    result +=
      text.slice(last, ref.from) +
      replacement(notebook, text, ref, depth, fromPath);
    last = ref.to;
  }
  return localImages(clean(result + text.slice(last)), fromPath);
}
