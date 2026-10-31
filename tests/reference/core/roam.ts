import {
  newId,
  blockText,
  safePagePath,
  pageTitle,
  pageName,
  parseBlocks,
  references,
  type Block,
} from "./roam-markdown";

type RecordValue = Record<string, unknown>;
export type RoamBlock = RecordValue & {
  string: string;
  uid: string;
  children?: RoamBlock[];
};
export type RoamPage = RecordValue & { title: string; children?: RoamBlock[] };
export type RoamMetadata = { version: 1; pages: Record<string, RoamPage> };
export type ImportedVault = {
  documents: { path: string; text: string }[];
  report: string[];
  metadata: RoamMetadata;
};
export type RoamExport = { pages: RoamPage[]; report: string[] };
const isRecord = (value: unknown): value is RecordValue =>
  !!value && typeof value === "object" && !Array.isArray(value);
const frontmatter = /^\uFEFF?---\r?\n([\s\S]*?)\r?\n---(?:\r?\n|$)/;
function portableTitle(original: string) {
  const cleaned = original
    .normalize("NFC")
    .trim()
    .replace(/[<>:"/\\|?*\p{Cc}]/gu, "-");
  let title = "";
  for (const character of cleaned) {
    if (
      title.length + character.length > 130 ||
      new TextEncoder().encode(title + character).length > 200
    )
      break;
    title += character;
  }
  title = title.replace(/[. ]+$/, "") || "Untitled";
  try {
    safePagePath(title);
  } catch {
    title = `Page ${title}`;
  }
  return title;
}
const heading = (node: RecordValue) =>
  Number.isInteger(node.heading) &&
  Number(node.heading) >= 1 &&
  Number(node.heading) <= 3
    ? Number(node.heading)
    : 0;
function listText(text: string, escape: boolean) {
  let fence: string | undefined;
  return text
    .split("\n")
    .map((line) => {
      const delimiter = /^ {0,3}(`{3,}|~{3,})(.*)$/.exec(line);
      if (delimiter) {
        if (!fence) fence = delimiter[1];
        else if (
          delimiter[1][0] === fence[0] &&
          delimiter[1].length >= fence.length &&
          !delimiter[2].trim()
        )
          fence = undefined;
        return line;
      }
      if (fence) return line;
      if (escape)
        return line.replace(
          /^( {0,3})([-+*]|\d+[.)])(?=[ \t])/,
          (_all, space: string, mark: string) =>
            space +
            (mark.length === 1
              ? `\\${mark}`
              : `${mark.slice(0, -1)}\\${mark.at(-1)}`),
        );
      return line
        .replace(/^( {0,3})\\([-+*])(?=[ \t])/, "$1$2")
        .replace(/^( {0,3}\d+)\\([.)])(?=[ \t])/, "$1$2");
    })
    .join("\n");
}
const markdownText = (node: RecordValue) =>
  listText(
    `${"#".repeat(heading(node))}${heading(node) ? " " : ""}${String(
      node.string,
    )
      .replace(/^\{\{(?:TODO|\[\[TODO\]\])\}\}/, "[ ]")
      .replace(/^\{\{(?:DONE|\[\[DONE\]\])\}\}/, "[x]")}`,
    true,
  );

function renderNodes(nodes: RoamBlock[] | undefined, depth = 0): string {
  if (!nodes) return "";
  if (depth > 128 || !Array.isArray(nodes))
    throw new Error(
      "Roam block nesting is invalid or exceeds 128 levels. Split the graph and retry.",
    );
  return nodes
    .map((node) => {
      if (
        !isRecord(node) ||
        typeof node.string !== "string" ||
        typeof node.uid !== "string"
      )
        throw new Error(
          "A Roam block is malformed. Export the graph again and retry.",
        );
      const indent = "  ".repeat(depth);
      const lines = markdownText(node).split(/\r?\n/);
      const text = lines
        .map((line, index) => `${indent}${index ? "  " : "- "}${line}`)
        .join("\n");
      return `${text}${lines.length > 1 ? `\n${indent}  ` : lines[0] ? " " : ""}^${node.uid}\n${renderNodes(node.children, depth + 1)}`;
    })
    .join("");
}

export function importRoamJSON(value: unknown): ImportedVault {
  if (!Array.isArray(value))
    throw new Error(
      "This is not a Roam JSON page export. Export the graph as JSON from Roam, then select that file.",
    );
  const ids = new Map<string, string>();
  const assigned = new WeakMap<object, string>();
  const report: string[] = [];
  const titles = new Set<string>();
  let blockCount = 0;
  const register = (node: RecordValue) => {
    const uid = node.uid;
    if (uid !== undefined && (typeof uid !== "string" || !uid))
      throw new Error(
        "A Roam UID is invalid. Export the graph again and retry.",
      );
    if (typeof uid === "string") {
      if (ids.has(uid))
        throw new Error(
          `Roam export has duplicate ID ${uid}. Fix the export before importing.`,
        );
      const local = /^[a-zA-Z0-9_-]+$/.test(uid) ? uid : newId();
      ids.set(uid, local);
      assigned.set(node, local);
      if (local !== uid)
        report.push(
          `Remapped block ID ${uid} to ${local} for Markdown compatibility.`,
        );
    } else assigned.set(node, newId());
  };
  const walk = (nodes: unknown, depth = 0) => {
    if (nodes === undefined) return;
    if (depth > 128 || !Array.isArray(nodes))
      throw new Error(
        "Roam block children must be an array with at most 128 nesting levels. Export the graph again and retry.",
      );
    for (const node of nodes) {
      if (!isRecord(node) || typeof node.string !== "string")
        throw new Error(
          "A Roam block is malformed or missing its text. Export the graph again and retry.",
        );
      register(node);
      blockCount++;
      walk(node.children, depth + 1);
    }
  };
  for (const page of value) {
    if (!isRecord(page) || typeof page.title !== "string" || !page.title.trim())
      throw new Error(
        "A Roam page has no title. Export the graph again and retry.",
      );
    if (titles.has(page.title))
      throw new Error(
        `Roam export repeats page title ${page.title}. Resolve the duplicate before importing.`,
      );
    titles.add(page.title);
    if (page.uid !== undefined) register(page);
    walk(page.children);
  }
  const rewrite = (text: string) => {
    for (const ref of references(text).reverse()) {
      if (ref.kind === "page" || ref.syntax !== "roam") continue;
      const id = ids.get(ref.target);
      if (!id) report.push(`Unresolved reference: ${ref.target}`);
      text =
        text.slice(0, ref.from) +
        text.slice(ref.from, ref.to).replace(ref.target, id ?? ref.target) +
        text.slice(ref.to);
    }
    return text;
  };
  const normalize = (
    nodes: RecordValue[] | undefined,
  ): RoamBlock[] | undefined =>
    nodes?.map((node) => {
      if (
        /\{\{(?!(?:pdf|\[\[pdf\]\]|embed|\[\[embed\]\]|TODO|DONE|\[\[(?:TODO|DONE)\]\]):?)/.test(
          String(node.string),
        )
      )
        report.push(
          `Preserved an unsupported Roam macro in ${node.uid ?? "a block"}.`,
        );
      if (/!\[[^\]]*\]\(https?:/.test(String(node.string)))
        report.push(
          `Remote image in ${node.uid ?? "a block"} still requires its original URL.`,
        );
      const result = {
        ...node,
        uid: assigned.get(node)!,
        string: rewrite(String(node.string)),
      } as RoamBlock;
      if (node.children !== undefined)
        result.children = normalize(node.children as RecordValue[]);
      return result;
    });
  const filenames = new Set<string>();
  const metadataPages: [string, RoamPage][] = [];
  const documents = value.map((input: RecordValue) => {
    const originalTitle = String(input.title);
    const source = { ...input, title: originalTitle } as RoamPage;
    if (input.uid !== undefined) source.uid = assigned.get(input)!;
    if (input.children !== undefined)
      source.children = normalize(input.children as RecordValue[]);
    const title = portableTitle(originalTitle);
    let candidate = title,
      index = 2;
    while (filenames.has(safePagePath(candidate).toLocaleLowerCase()))
      candidate = `${title} ${index++}`;
    filenames.add(safePagePath(candidate).toLocaleLowerCase());
    if (safePagePath(candidate) !== `${originalTitle}.md`)
      report.push(
        `Stored ${originalTitle} as ${safePagePath(candidate)}; retained the original title in front matter.`,
      );
    const text = `---\ntitle: ${JSON.stringify(originalTitle)}\n---\n\n${renderNodes(source.children)}`;
    if (new TextEncoder().encode(text).length > 10 * 1024 * 1024)
      throw new Error(
        `${originalTitle} exceeds the 10 MB note limit. Split that page in Roam and export again.`,
      );
    const path = safePagePath(candidate);
    metadataPages.push([path, source]);
    return { path, text };
  });
  report.unshift(
    `Imported ${documents.length} pages and ${blockCount} blocks. Original export unchanged.`,
    "Original Roam page and block metadata are retained in .quire/roam.json. Executable macros are preserved as text; remote attachments are not downloaded.",
  );
  return {
    documents,
    report: [...new Set(report)],
    metadata: { version: 1, pages: Object.fromEntries(metadataPages) },
  };
}

type PageInfo = {
  path: string;
  text: string;
  title: string;
  source?: RoamPage;
  unchanged: boolean;
  blocks: Block[];
  ids: Map<string, string>;
};

export function exportRoamJSON(
  documents: ReadonlyArray<{ path: string; text: string }>,
  metadata?: RoamMetadata,
): RoamExport {
  const report: string[] = [];
  const identity = new Map<string, { uid: string; path: string }>();
  const used = new Set<string>();
  const titles = new Set<string>();
  let generated = 0;
  if (metadata && (metadata.version !== 1 || !isRecord(metadata.pages)))
    throw new Error(
      "The Roam metadata file is invalid. Restore .quire/roam.json from a backup before exporting.",
    );
  for (const source of Object.values(metadata?.pages ?? {})) {
    try {
      if (!isRecord(source) || typeof source.title !== "string")
        throw new Error("Invalid page");
      renderNodes(source.children);
    } catch {
      throw new Error(
        "The Roam metadata file contains an invalid page. Restore .quire/roam.json from a backup before exporting.",
      );
    }
  }
  const register = (uid: string, path: string) => {
    if (used.has(uid))
      throw new Error(
        `Duplicate block or page ID ${uid} prevents a reliable Roam export. Give each block a unique identifier in Source.`,
      );
    used.add(uid);
    identity.set(uid, { uid, path });
    return uid;
  };
  const pages: PageInfo[] = documents.map((doc) => {
    const text = doc.text.replaceAll("\r\n", "\n");
    const title = pageTitle(doc.path, text);
    if (titles.has(title))
      throw new Error(
        `Multiple pages are titled ${title}. Give them distinct titles before exporting to Roam.`,
      );
    titles.add(title);
    const source =
      metadata &&
      (Object.hasOwn(metadata.pages, doc.path)
        ? metadata.pages[doc.path]
        : Object.values(metadata.pages).find((p) => p.title === title));
    if (source) {
      try {
        if (!isRecord(source) || typeof source.title !== "string")
          throw new Error("Invalid page");
        renderNodes(source.children);
      } catch {
        throw new Error(
          `${doc.path} has invalid Roam metadata. Restore .quire/roam.json from a backup before exporting.`,
        );
      }
    }
    const body = text
      .slice(frontmatter.exec(text)?.[0].length ?? 0)
      .replace(/^\n/, "");
    return {
      ...doc,
      text,
      title,
      source,
      unchanged: !!source && body === renderNodes(source.children),
      blocks: parseBlocks(text),
      ids: new Map(),
    };
  });
  const originalNodes = new Map<string, RoamBlock>();
  const collect = (
    nodes: RoamBlock[] | undefined,
    page: PageInfo,
    track: boolean,
  ) => {
    for (const node of nodes ?? []) {
      originalNodes.set(node.uid, node);
      if (track) register(node.uid, page.path);
      collect(node.children, page, track);
    }
  };
  for (const page of pages) {
    if (typeof page.source?.uid === "string") {
      const uid = String(page.source.uid);
      if (used.has(uid))
        throw new Error(
          `Duplicate page ID ${uid}. Resolve it before exporting.`,
        );
      used.add(uid);
      identity.set(uid, { uid, path: page.path });
    }
    collect(page.source?.children, page, page.unchanged);
    if (!page.unchanged)
      for (const block of page.blocks) {
        const uid = block.id ?? newId();
        if (!block.id) generated++;
        page.ids.set(block.key, register(uid, page.path));
      }
  }
  const findPage = (target: string) =>
    pages.find((p) => p.title === target) ??
    pages.find((p) => pageName(p.path) === target.replace(/\.md$/i, "")) ??
    pages.find((p) =>
      [pageName(p.path), p.title].some(
        (n) =>
          n.toLocaleLowerCase() ===
          target.replace(/\.md$/i, "").toLocaleLowerCase(),
      ),
    );
  const rewrite = (text: string, page: PageInfo) => {
    for (const ref of references(text).reverse()) {
      let replacement: string | undefined;
      if (ref.media) continue;
      if (ref.kind === "page") {
        const target = findPage(ref.target);
        if (target)
          replacement = ref.label
            ? `[${ref.label}]([[${target.title}]])`
            : `[[${target.title}]]`;
      } else if (
        ref.kind === "embed" &&
        ref.syntax === "wiki" &&
        !ref.target.includes("#^")
      ) {
        const target = findPage(ref.target);
        if (target) replacement = `{{[[embed]]: [[${target.title}]]}}`;
        else if (
          /\.(png|jpe?g|gif|webp|svg|avif|pdf|mp3|mp4|wav)$/i.test(ref.target)
        )
          replacement = `![](${ref.target})`;
        else
          report.push(`Unresolved page embed in ${page.path}: ${ref.target}`);
      } else {
        const separator = ref.target.indexOf("#^");
        const id =
          separator >= 0 ? ref.target.slice(separator + 2) : ref.target;
        const target = identity.get(id);
        if (
          target &&
          (separator < 0 ||
            findPage(ref.target.slice(0, separator))?.path === target.path)
        )
          replacement =
            ref.kind === "embed"
              ? `{{[[embed]]: ((${target.uid}))}}`
              : `((${target.uid}))`;
        else report.push(`Unresolved reference in ${page.path}: ${ref.target}`);
      }
      if (replacement)
        text = text.slice(0, ref.from) + replacement + text.slice(ref.to);
    }
    if (/!?\[[^\]]*\]\((?!https?:|mailto:|data:|#)[^)]+\)/.test(text))
      report.push(
        `Local attachment or relative link in ${page.path} remains a dependency. Copy its files separately; JSON does not contain attachment bytes.`,
      );
    return text;
  };
  const copyOriginal = (
    nodes: RoamBlock[] | undefined,
    page: PageInfo,
  ): RoamBlock[] | undefined =>
    nodes?.map((node) => {
      const result = { ...node, string: rewrite(node.string, page) };
      if (node.children) result.children = copyOriginal(node.children, page);
      return result;
    });
  const output = pages.map((page) => {
    if (page.unchanged) {
      const result = { ...page.source!, title: page.title };
      if (page.source!.children)
        result.children = copyOriginal(page.source!.children, page);
      return result;
    }
    const roots: RoamBlock[] = [];
    const built = new Map<string, RoamBlock>();
    let cursor = frontmatter.exec(page.text)?.[0].length ?? 0;
    const gap = (to: number) => {
      const text = page.text.slice(cursor, to).trim();
      if (text) {
        const uid = newId();
        generated++;
        roots.push({ uid, string: rewrite(text, page) });
      }
    };
    for (const block of page.blocks) {
      if (!block.parent) {
        gap(block.from);
        cursor = block.subtreeTo;
      }
      const uid = page.ids.get(block.key)!;
      const original = originalNodes.get(uid);
      const content = blockText(page.text, block);
      const unchanged =
        original && content.trimEnd() === markdownText(original).trimEnd();
      const attributes = { ...original } as RecordValue;
      delete attributes.children;
      delete attributes.string;
      delete attributes.uid;
      delete attributes.heading;
      if (!unchanged) {
        delete attributes["edit-time"];
        delete attributes["edit-user"];
        delete attributes["edit-email"];
      }
      const level = /^(#{1,3})\s/.exec(content);
      const current = original ? listText(content, false) : content;
      const currentLevel = /^(#{1,3})\s/.exec(current);
      const string = unchanged
        ? original.string
        : (currentLevel ? current.slice(currentLevel[0].length) : current)
            .replace(/^\[ \](?=\s|$)/, "{{[[TODO]]}}")
            .replace(/^\[[xX]\](?=\s|$)/, "{{[[DONE]]}}");
      const node: RoamBlock = {
        ...attributes,
        uid,
        string: rewrite(string, page),
      };
      if (unchanged && original.heading !== undefined)
        node.heading = original.heading;
      else if (level) node.heading = level[1].length;
      built.set(block.key, node);
      const parent = block.parent ? built.get(block.parent) : undefined;
      if (parent) (parent.children ??= []).push(node);
      else roots.push(node);
    }
    gap(page.text.length);
    const metadata = frontmatter.exec(page.text)?.[1];
    if (
      !page.source &&
      metadata
        ?.split("\n")
        .some((line) => line.trim() && !line.startsWith("title:"))
    ) {
      roots.unshift({
        uid: newId(),
        string: `\`\`\`yaml\n${metadata}\n\`\`\``,
      });
      generated++;
      report.push(
        `Preserved custom Markdown front matter from ${page.path} as a YAML code block.`,
      );
    }
    return { ...page.source, title: page.title, children: roots };
  });
  report.unshift(
    `Exported ${output.length} pages to Roam JSON. Existing block IDs are preserved; ${generated} new export-only IDs were generated.`,
  );
  return { pages: output, report: [...new Set(report)] };
}
