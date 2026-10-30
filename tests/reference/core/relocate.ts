// Extracted from rt src/core/relocate.ts at 5ea5c22310ff (last changed in 35e673677291). The
// body of planRelocation is unchanged. Imports point at the frozen copies, and the two types it
// imported become local declarations of exactly the members it reads: Notebook from store.ts and
// FileEdit from types.ts.
import {
  parseMarkdown,
  pageName,
  pageTitle,
  references,
  relativeVaultPath,
} from "../markdown/markdown";
import { notebookPath, relativePath } from "./file-paths";
export type FileEdit = {
  remove?: boolean;
  retirement?: string;
  historyFrom?: string;
  path: string;
  text: string;
  expectedRevision: string | null;
};
export interface Notebook {
  documents: Map<string, { revision: string | null; state: { lineBreak: string } }>;
  pagePaths(): string[];
  title(path: string): string;
  findPage(target: string): string | undefined;
  all(): { path: string; text: string }[];
}
export type RelocationPlan = {
  source: string;
  target: string;
  edits: FileEdit[];
  observed: Map<string, string>;
  updates: { path: string; before: string; after: string }[];
};
// A move keeps the page's title (front matter records it when the filename
// changes), so links by title need no edit. A rename changes the title
// too: every link follows, and a title in front matter takes the new name.
export function planRelocation(
  notebook: Notebook,
  source: string,
  target: string,
  { rename = false }: { rename?: boolean } = {},
): RelocationPlan {
  target = notebookPath(target, true);
  if (!notebook.documents.has(source))
    throw new Error("This source page is missing. Reopen it before moving.");
  if (target.toLocaleLowerCase() === source.toLocaleLowerCase())
    throw new Error(
      "Choose a different path. Case-only renames need an intermediate filename on macOS and Windows.",
    );
  if (
    notebook
      .pagePaths()
      .some((path) => path.toLocaleLowerCase() === target.toLocaleLowerCase())
  )
    throw new Error(
      "A file already uses that destination. Choose another path.",
    );
  const sourceTitle = notebook.title(source),
    uniqueTitle =
      notebook
        .pagePaths()
        .filter(
          (path) =>
            notebook.title(path).toLocaleLowerCase() ===
            sourceTitle.toLocaleLowerCase(),
        ).length === 1;
  const edits: FileEdit[] = [],
    updates: RelocationPlan["updates"] = [],
    observed = new Map<string, string>();
  for (const document of notebook.all()) {
    const moved = document.path === source,
      destination = moved ? target : document.path,
      changes: { from: number; to: number; insert: string }[] = [],
      text = document.text;
    for (const reference of references(text)) {
      const hash = reference.target.indexOf("#"),
        base = hash < 0 ? reference.target : reference.target.slice(0, hash),
        fragment = hash < 0 ? "" : reference.target.slice(hash);
      if (
        (reference.syntax === "wiki" || hash >= 0) &&
        notebook.findPage(base) === source
      ) {
        if (
          !rename &&
          uniqueTitle &&
          base.toLocaleLowerCase() === sourceTitle.toLocaleLowerCase()
        )
          continue;
        const replacement = pageName(target) + fragment;
        changes.push({
          from: reference.from,
          to: reference.to,
          insert: text
            .slice(reference.from, reference.to)
            .replace(reference.target, replacement),
        });
      } else if (
        moved &&
        reference.kind === "embed" &&
        relativeVaultPath(source, reference.target) &&
        !notebook.findPage(base) &&
        !/^\w+:/.test(reference.target)
      ) {
        const absolute = relativeVaultPath(source, reference.target)!;
        changes.push({
          from: reference.from,
          to: reference.to,
          insert: text
            .slice(reference.from, reference.to)
            .replace(
              reference.target,
              relativePath(target, absolute) + fragment,
            ),
        });
      }
    }
    parseMarkdown(text).iterate({
      enter(node) {
        if (node.name !== "URL") return;
        const raw = text.slice(node.from, node.to),
          angle = raw.startsWith("<") && raw.endsWith(">"),
          url = angle ? raw.slice(1, -1) : raw;
        if (/^[a-z][a-z0-9+.-]*:|^\/\//i.test(url)) return;
        const absolute = relativeVaultPath(document.path, url);
        if (!absolute) return;
        const fragment = url.includes("#") ? url.slice(url.indexOf("#")) : "";
        if (!moved && absolute !== source) return;
        const replacement =
          relativePath(destination, absolute === source ? target : absolute) +
          fragment;
        changes.push({
          from: node.from,
          to: node.to,
          insert: angle
            ? "<" + replacement + ">"
            : replacement.replaceAll(" ", "%20"),
        });
      },
    });
    for (const change of changes) {
      const before = text.slice(change.from, change.to);
      if (before !== change.insert)
        updates.push({ path: document.path, before, after: change.insert });
    }
    let next = text;
    for (const change of changes.sort((a, b) => b.from - a.from))
      next = next.slice(0, change.from) + change.insert + next.slice(change.to);
    if (moved && rename)
      next = next.replace(
        /^(\uFEFF?---\r?\n(?:(?!---)[^\n]*\n)*?)title:[^\n]*/,
        (_, head: string) =>
          `${head}title: ${JSON.stringify(pageName(target))}`,
      );
    else if (moved) {
      const title = pageTitle(source, text),
        metadata = /^\uFEFF?---\r?\n([\s\S]*?)\r?\n---(?:\r?\n|$)/.exec(next);
      if (!metadata)
        next = `---\ntitle: ${JSON.stringify(title)}\n---\n\n${next}`;
      else if (!/^title:/m.test(metadata[1]))
        next = next.replace(
          /^(\uFEFF?---\r?\n)/,
          `$1title: ${JSON.stringify(title)}\n`,
        );
    }
    if (moved || next !== text) {
      const entry = notebook.documents.get(document.path)!;
      observed.set(document.path, text);
      edits.push({
        path: destination,
        text:
          entry.state.lineBreak === "\r\n"
            ? next.replaceAll("\n", "\r\n")
            : next,
        expectedRevision: moved ? null : entry.revision,
        ...(moved ? { historyFrom: source } : {}),
      });
    }
  }
  edits.sort((a, b) => (a.path === target ? -1 : b.path === target ? 1 : 0));
  edits.push({
    path: source,
    text: "",
    expectedRevision: notebook.documents.get(source)!.revision,
    remove: true,
  });
  return { source, target, edits, observed, updates };
}
