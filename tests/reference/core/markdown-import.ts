// The planning in rt NotebookEngine.importMarkdown (src/desktop/engine/engine.ts). The source
// folder becomes an in-memory tree listed in readdir order, store.pageByPath becomes a set of
// path keys (sqlite-store pathKey), and this.write records the page instead of saving it. The
// walk, the 500-page transactions and the skip rule are unchanged.
import { notebookPath } from "./file-paths";

export type Entry = { name: string; children?: Entry[]; file?: boolean };
const pathKey = (path: string) => path.normalize("NFC").toLowerCase();

export function importMarkdown(source: Entry[], existing: string[]) {
  const pages: { path: string; text: string }[] = [];
  const files: string[] = [];
  const at = (relative: string) =>
    relative
      ? relative
          .split("/")
          .reduce(
            (entries, name) => entries.find((entry) => entry.name === name)!.children!,
            source,
          )
      : source;
  const walk = (relative: string) => {
    for (const entry of at(relative)) {
      if (entry.name.startsWith(".")) continue;
      const child = relative ? `${relative}/${entry.name}` : entry.name;
      if (entry.children) walk(child);
      else if (!entry.file) continue;
      else if (/\.md$/i.test(entry.name)) pages.push({ path: child, text: "" });
      else files.push(child);
    }
  };
  walk("");
  const keys = new Set(existing.map(pathKey));
  const written: string[] = [];
  let imported = 0;
  for (let start = 0; start < pages.length; start += 500)
    for (const page of pages.slice(start, start + 500)) {
      const target = notebookPath(page.path);
      if (keys.has(pathKey(target))) continue;
      keys.add(pathKey(target));
      written.push(target);
      imported++;
    }
  return {
    pages: imported,
    skipped: pages.length - imported,
    files,
    written,
    sources: pages.map((page) => page.path),
  };
}
