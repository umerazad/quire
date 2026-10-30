import { parser, GFM } from "@lezer/markdown";
import { TreeFragment, type SyntaxNode, type Tree } from "@lezer/common";
import { RangeSet } from "./range-set";

export type Block = {
  key: string;
  id?: string;
  from: number;
  to: number;
  contentFrom: number;
  contentTo: number;
  subtreeTo: number;
  depth: number;
  kind: "list" | "heading" | "code" | "paragraph" | "quote";
  text: string;
  parent?: string;
};
export type Reference = {
  kind: "page" | "block" | "embed";
  syntax: "wiki" | "roam";
  target: string;
  from: number;
  to: number;
  label?: string;
  media?: "pdf";
};
// A "-" typed under a line must start a list or rule, not make a setext heading.
export const noSetextHeadings = { remove: ["SetextHeading"] };
export const markdownParser = parser.configure([GFM, noSetextHeadings]);
const recentTrees: {
  text: string;
  tree: Tree;
  fragments: readonly TreeFragment[];
}[] = [];
// Parses Markdown, reusing the last trees: an identical text (the editor and
// the store parse each edit) is a cache hit, and a small edit to a large page
// re-parses only the changed region.
export function parseMarkdown(text: string): Tree {
  const hit = recentTrees.findIndex((entry) => entry.text === text);
  if (hit >= 0) {
    const [entry] = recentTrees.splice(hit, 1);
    recentTrees.unshift(entry);
    return entry.tree;
  }
  const base = recentTrees[0];
  let fragments: readonly TreeFragment[] | undefined;
  if (base && text.length > 4096) {
    const { from, to, insert } = minimalChange(base.text, text);
    if (insert.length < text.length / 2)
      fragments = TreeFragment.applyChanges(base.fragments, [
        { fromA: from, toA: to, fromB: from, toB: from + insert.length },
      ]);
  }
  const tree = markdownParser.parse(text, fragments);
  recentTrees.unshift({ text, tree, fragments: TreeFragment.addTree(tree) });
  recentTrees.length = Math.min(recentTrees.length, 4);
  return tree;
}
// Text as the editor holds it: CodeMirror stores every line break as \n.
export const editorText = (text: string) =>
  text.includes("\r") ? text.replace(/\r\n?/g, "\n") : text;
export const idPattern = /(?:^|[ \t])\^([a-zA-Z0-9_-]+)[ \t]*$/;
export const stripId = (text: string) => text.replace(idPattern, "").trimEnd();
export function blockText(text: string, block: Block) {
  if (block.kind !== "list") return block.text;
  const lineFrom = text.lastIndexOf("\n", block.from - 1) + 1,
    indent = block.contentFrom - lineFrom;
  return block.text
    .split("\n")
    .map((line, index) =>
      index
        ? line.slice(Math.min(indent, /^[ \t]*/.exec(line)![0].length))
        : line,
    )
    .join("\n");
}
export const pageTitle = (path: string, text = "") => {
  const metadata = /^\uFEFF?---\r?\n([\s\S]*?)\r?\n---(?:\r?\n|$)/.exec(
    text,
  )?.[1];
  const title = metadata && /^title:\s*(.+)$/m.exec(metadata)?.[1];
  if (title) {
    try {
      return String(JSON.parse(title));
    } catch {
      return title.replace(/^['"]|['"]$/g, "");
    }
  }
  return path.replace(/\.md$/i, "").split("/").at(-1)!;
};
export const pageName = (path: string) => path.replace(/\.md$/i, "");
export function relativeVaultPath(from: string | undefined, target: string) {
  let decoded: string;
  try {
    decoded = decodeURIComponent(target.split("#")[0]);
  } catch {
    return undefined;
  }
  const parts = decoded.startsWith("/")
    ? []
    : (from?.split("/").slice(0, -1) ?? []);
  for (const part of decoded.split("/")) {
    if (!part || part === ".") continue;
    if (part === "..") {
      if (!parts.length) return undefined;
      parts.pop();
    } else parts.push(part);
  }
  return parts.join("/");
}
export const newId = () => `b-${crypto.randomUUID().replaceAll("-", "")}`;
export const safePagePath = (title: string) => {
  const name = title.trim().normalize("NFC");
  if (
    !name ||
    name.length > 150 ||
    /[<>:"/\\|?*\p{Cc}]/u.test(name) ||
    /[. ]$/.test(name) ||
    name.startsWith(".") ||
    /^(con|prn|aux|nul|com[0-9]|lpt[0-9])(?:\.|$)/i.test(name) ||
    name === ".." ||
    name === "."
  )
    throw new Error(
      "This page name cannot be used on Windows and macOS. Use a shorter name without a leading dot, slashes, or reserved filename characters.",
    );
  return `${name}.md`;
};

export const frontmatterLength = (text: string) =>
  /^\uFEFF?---\r?\n[\s\S]*?\r?\n---(?:\r?\n|$)/.exec(text)?.[0].length ?? 0;
// The tags a page's YAML front matter lists, without "#", in the forms
// Obsidian accepts: "tags: [a, b]", "tags: a, b" or a "- a" list.
export function frontmatterTags(text: string): string[] {
  const length = frontmatterLength(text);
  if (!length) return [];
  const lines = text.slice(0, length).split(/\r?\n/),
    result: string[] = [];
  const add = (value: string) => {
    const tag = value
      .trim()
      .replace(/^["']|["']$/g, "")
      .replace(/^#/, "")
      .trim();
    if (tag) result.push(tag);
  };
  for (let index = 0; index < lines.length; index++) {
    const match = /^(tags?)[ \t]*:[ \t]*(.*)$/i.exec(lines[index]);
    if (!match) continue;
    const inline = match[2].trim();
    if (inline)
      inline
        .replace(/^\[|\]$/g, "")
        .split(/[,\s]+/)
        .forEach(add);
    else
      for (
        let next = index + 1;
        next < lines.length && /^[ \t]*-[ \t]+/.test(lines[next]);
        next++
      )
        add(lines[next].replace(/^[ \t]*-[ \t]+/, ""));
  }
  return result;
}
// Results derived from a parsed tree, shared by the editor and the notebook
// index, which analyze the same text after each edit. Callers must not mutate them.
const derived = new WeakMap<
  Tree,
  { blocks?: Block[]; references?: Reference[]; tags?: TagToken[] }
>();
const derivedFrom = (tree: Tree) => {
  let value = derived.get(tree);
  if (!value) derived.set(tree, (value = {}));
  return value;
};
export function parseBlocks(
  text: string,
  tree: Tree = parseMarkdown(text),
): Block[] {
  return (derivedFrom(tree).blocks ??= blocksOf(text, tree));
}
function blocksOf(text: string, tree: Tree): Block[] {
  const result: Block[] = [];
  const frontmatter = frontmatterLength(text);
  const visit = (node: SyntaxNode, depth: number, parent?: string) => {
    if (node.to <= frontmatter) return;
    if (["Document", "BulletList", "OrderedList"].includes(node.name)) {
      for (let child = node.firstChild; child; child = child.nextSibling)
        visit(child, depth, parent);
      return;
    }
    if (node.name === "ListItem") {
      const mark = node.getChild("ListMark");
      let contentFrom = mark?.to ?? node.from;
      while (text[contentFrom] === " " || text[contentFrom] === "\t")
        contentFrom++;
      const nested: SyntaxNode[] = [];
      for (let child = node.firstChild; child; child = child.nextSibling)
        if (["BulletList", "OrderedList"].includes(child.name))
          nested.push(child);
      let contentTo = nested[0]?.from ?? node.to;
      while (contentTo > contentFrom && /\s/.test(text[contentTo - 1]))
        contentTo--;
      const content = text.slice(contentFrom, contentTo);
      const id = idPattern.exec(content)?.[1];
      const key = id ?? `at-${node.from}`;
      result.push({
        key,
        id,
        from: node.from,
        to: contentTo,
        contentFrom,
        contentTo,
        subtreeTo: node.to,
        depth,
        kind: "list",
        text: stripId(content),
        parent,
      });
      nested.forEach((child) => visit(child, depth + 1, key));
      return;
    }
    if (node.name === "HorizontalRule") return;
    const kind = node.name.startsWith("ATXHeading")
      ? "heading"
      : ["FencedCode", "CodeBlock"].includes(node.name)
        ? "code"
        : node.name === "Blockquote"
          ? "quote"
          : "paragraph";
    const content = text.slice(node.from, node.to);
    const id = kind === "code" ? undefined : idPattern.exec(content)?.[1];
    result.push({
      key: id ?? `at-${node.from}`,
      id,
      from: node.from,
      to: node.to,
      contentFrom: node.from,
      contentTo: node.to,
      subtreeTo: node.to,
      depth,
      kind,
      text: stripId(content),
      parent,
    });
  };
  visit(tree.topNode, 0);
  return result;
}

const noReferences: Reference[] = [];
export function references(text: string, tree?: Tree): Reference[] {
  if (!/\(\(|\[\[|\{\{pdf:/.test(text)) return noReferences;
  tree ??= parseMarkdown(text);
  return (derivedFrom(tree).references ??= referencesOf(text, tree));
}
function referencesOf(text: string, tree: Tree): Reference[] {
  const ignored: [number, number][] = [];
  const metadata = frontmatterLength(text);
  if (metadata) ignored.push([0, metadata]);
  tree.iterate({
    enter(node) {
      if (["FencedCode", "CodeBlock", "InlineCode"].includes(node.name)) {
        ignored.push([node.from, node.to]);
        return false;
      }
    },
  });
  const result: Reference[] = [];
  for (const match of text.matchAll(
    /\{\{(?:pdf|\[\[pdf\]\]):\s*([^}\n]+?)\s*\}\}/g,
  )) {
    const from = match.index;
    if (ignored.some(([start, end]) => from >= start && from < end)) continue;
    result.push({
      kind: "embed",
      syntax: "wiki",
      target: match[1].trim(),
      media: "pdf",
      from,
      to: from + match[0].length,
    });
  }
  const pdfRanges = [...result];
  const expression =
    /\{\{(?:embed|\[\[embed\]\]):\s*(?:\(\(([^)\n]+)\)\)|\[\[([^\]\n]+)\]\])\s*\}\}|!\[\[([^\]\n]+)\]\]|\(\(([^)\n]+)\)\)|\[\[([^\]\n]+)\]\]/g;
  for (const match of text.matchAll(expression)) {
    const from = match.index!;
    if (ignored.some(([start, end]) => from >= start && from < end)) continue;
    if (pdfRanges.some((ref) => from >= ref.from && from < ref.to)) continue;
    const raw = match[1] ?? match[2] ?? match[3] ?? match[4] ?? match[5];
    const [target, label] = raw.split("|");
    result.push({
      kind:
        match[1] || match[2] || match[3]
          ? "embed"
          : match[4] || target.includes("#^")
            ? "block"
            : "page",
      syntax: match[2] || match[3] || match[5] ? "wiki" : "roam",
      target,
      label,
      from,
      to: from + match[0].length,
    });
  }
  return result.sort((a, b) => a.from - b.from);
}

export function minimalChange(before: string, after: string) {
  let from = 0;
  while (
    from < before.length &&
    from < after.length &&
    before[from] === after[from]
  )
    from++;
  let end = before.length,
    nextEnd = after.length;
  while (
    end > from &&
    nextEnd > from &&
    before[end - 1] === after[nextEnd - 1]
  ) {
    end--;
    nextEnd--;
  }
  return { from, to: end, insert: after.slice(from, nextEnd) };
}

export function tagExcludedRanges(
  text: string,
  tree: Tree = parseMarkdown(text),
  refs = references(text, tree),
) {
  const ignored: [number, number][] = refs.map((ref) => [ref.from, ref.to]);
  const metadata = frontmatterLength(text);
  if (metadata) ignored.push([0, metadata]);
  tree.iterate({
    enter(node) {
      if (
        [
          "FencedCode",
          "CodeBlock",
          "InlineCode",
          "Link",
          "Image",
          "Autolink",
          "HTMLBlock",
          "HTMLTag",
          "Escape",
          "LinkReference",
        ].includes(node.name)
      ) {
        ignored.push([node.from, node.to]);
        return false;
      }
    },
  });
  return ignored;
}
export function tagCompletionRange(
  text: string,
  position: number,
  tree: Tree = parseMarkdown(text),
) {
  const match = /(^|[\s([{"'*~])#([\p{L}\p{N}_/-]*)$/u.exec(
    text.slice(0, position),
  );
  if (!match) return;
  const from = position - match[2].length - 1;
  if (
    tagExcludedRanges(text, tree).some(
      ([start, end]) => from >= start && from < end,
    )
  )
    return;
  const tail = /^[\p{L}\p{N}_/-]*/u.exec(text.slice(position))![0];
  return { from, to: position + tail.length };
}
export type TagToken = { from: number; to: number; text: string };
export function tags(
  text: string,
  tree: Tree = parseMarkdown(text),
  refs = references(text, tree),
): TagToken[] {
  if (!text.includes("#")) return [];
  if (refs !== references(text, tree)) return tagsOf(text, tree, refs);
  return (derivedFrom(tree).tags ??= tagsOf(text, tree, refs));
}
function tagsOf(text: string, tree: Tree, refs: Reference[]): TagToken[] {
  const ignored = new RangeSet();
  for (const [from, to] of tagExcludedRanges(text, tree, refs))
    ignored.add(from, to);
  const result: TagToken[] = [];
  const pattern =
    /(^|[\s([{"'*~])#([\p{L}\p{N}_][\p{L}\p{N}_-]*(?:\/[\p{L}\p{N}_][\p{L}\p{N}_-]*)*)/gu;
  for (const match of text.matchAll(pattern)) {
    const from = match.index + match[1].length;
    const to = from + match[2].length + 1;
    if (!ignored.overlaps(from, to))
      result.push({ from, to, text: text.slice(from, to) });
  }
  return result;
}

export function copyWithNewIds(
  text: string,
  destination: string,
  sourceNames: string[] = [],
) {
  const blocks = parseBlocks(text);
  const ids = new Map(
    blocks.flatMap((block) => (block.id ? [[block.id, newId()] as const] : [])),
  );
  const edits: { from: number; to: number; insert: string }[] = [];
  for (const block of blocks)
    if (block.id) {
      const marker = idPattern.exec(
        text.slice(block.contentFrom, block.contentTo),
      )!;
      const from =
        block.contentFrom + marker.index + marker[0].indexOf("^") + 1;
      edits.push({
        from,
        to: from + block.id.length,
        insert: ids.get(block.id)!,
      });
    }
  for (const reference of references(text)) {
    if (reference.kind === "page") continue;
    const separator = reference.target.indexOf("#^");
    const id =
      separator >= 0 ? reference.target.slice(separator + 2) : reference.target;
    if (
      !ids.has(id) ||
      (separator >= 0 &&
        !sourceNames.includes(reference.target.slice(0, separator)))
    )
      continue;
    const target =
      separator >= 0
        ? `${pageName(destination)}#^${ids.get(id)}`
        : ids.get(id)!;
    edits.push({
      from: reference.from,
      to: reference.to,
      insert: text
        .slice(reference.from, reference.to)
        .replace(reference.target, target),
    });
  }
  return edits
    .sort((a, b) => b.from - a.from)
    .reduce(
      (current, edit) =>
        current.slice(0, edit.from) + edit.insert + current.slice(edit.to),
      text,
    );
}

export function resolveBlock(
  documents: Iterable<{ path: string; text: string }>,
  target: string,
): { path: string; block: Block } | { error: string } {
  const separator = target.indexOf("#^");
  const id = separator >= 0 ? target.slice(separator + 2) : target;
  const page = separator >= 0 ? target.slice(0, separator) : undefined;
  const matches: { path: string; block: Block }[] = [];
  for (const doc of documents) {
    if (
      page &&
      ![pageName(doc.path), pageTitle(doc.path, doc.text)].some(
        (value) => value.toLocaleLowerCase() === page.toLocaleLowerCase(),
      )
    )
      continue;
    for (const block of parseBlocks(doc.text))
      if (block.id === id) matches.push({ path: doc.path, block });
  }
  return matches.length === 1
    ? matches[0]
    : {
        error: matches.length
          ? `Multiple blocks use ${id}. Give the copied block a new identifier in source view.`
          : `Block ${id} is missing. Restore its source or update this reference.`,
      };
}
