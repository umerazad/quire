import {
  activeDailyLayout,
  defaultDailyLayout,
  parseDaily,
} from "./daily-layout";
export type BrowserKind =
  "pages" | "tags" | "daily" | "tasks" | "help" | "readme" | "ai";
export type Destination =
  | { kind: "page"; path: string }
  | { kind: "tag"; tag: string }
  | { kind: "browser"; view: BrowserKind };
export const tagSorts = ["name", "most", "fewest"] as const;
export type TagSort = (typeof tagSorts)[number];
// Projects with an open action are active; the rest are inactive.
export const projectStatuses = ["active", "inactive", "all"] as const;
export type ProjectStatus = (typeof projectStatuses)[number];
export type BrowserView = {
  query: string;
  limit: number;
  namespace?: string;
  month?: string;
  // Tag order: by name, or by how many blocks use the tag.
  tagSort?: TagSort;
  taskView?: import("./tasks").TaskState | "projects" | "due" | "upcoming";
  taskProject?: string;
  taskDraft?: string;
  projectStatus?: ProjectStatus;
};
export type TaskStatus = "all" | "open" | "due" | "done";
export type Workspace = {
  filesOpen?: boolean;
  sidePath?: string;
  tagViews?: Record<
    string,
    { query: string; limit: number; scroll: number; taskStatus?: TaskStatus }
  >;
  browsers?: Record<string, BrowserView>;
  expanded: string[];
  favorites: Destination[];
  recent: Destination[];
  tabs: Destination[];
  active?: Destination;
  positions: Record<string, number>;
  // Pages whose linked references list the oldest daily pages first.
  oldestFirst?: string[];
};
export const destinationKey = (value: Destination) =>
  value.kind === "page"
    ? `page:${value.path}`
    : value.kind === "tag"
      ? `tag:${value.tag}`
      : `browser:${value.view}`;
export const emptyWorkspace = (): Workspace => ({
  expanded: [],
  favorites: [],
  recent: [],
  tabs: [],
  positions: {},
});
export function loadWorkspace(root: string): Workspace {
  try {
    const value = JSON.parse(
      localStorage.getItem("quire-workspace:" + root) ?? "null",
    );
    if (!value || !Array.isArray(value.tabs)) return emptyWorkspace();
    // Help opens beside the page now, so older saved Help tabs are dropped.
    const valid = (item: unknown): item is Destination =>
      !!item &&
      typeof item === "object" &&
      (("kind" in item &&
        item.kind === "page" &&
        "path" in item &&
        typeof item.path === "string") ||
        ("kind" in item &&
          item.kind === "tag" &&
          "tag" in item &&
          typeof item.tag === "string") ||
        ("kind" in item &&
          item.kind === "browser" &&
          "view" in item &&
          ["pages", "tags", "daily", "tasks", "ai"].includes(
            String(item.view),
          )));
    const validView = (item: unknown): item is BrowserView =>
      !!item &&
      typeof item === "object" &&
      "query" in item &&
      typeof item.query === "string" &&
      "limit" in item &&
      typeof item.limit === "number" &&
      Number.isInteger(item.limit) &&
      item.limit > 0 &&
      item.limit <= 10000 &&
      (!("taskDraft" in item) || typeof item.taskDraft === "string") &&
      (!("tagSort" in item) ||
        (tagSorts as readonly unknown[]).includes(item.tagSort)) &&
      (!("taskProject" in item) || typeof item.taskProject === "string") &&
      (!("projectStatus" in item) ||
        (projectStatuses as readonly unknown[]).includes(item.projectStatus)) &&
      (!("taskView" in item) ||
        [
          "inbox",
          "next",
          "waiting",
          "someday",
          "projects",
          "done",
          "due",
          "upcoming",
        ].includes(String(item.taskView))) &&
      (!("taskStatus" in item) ||
        ["all", "open", "due", "done"].includes(String(item.taskStatus)));
    const browsers = Object.fromEntries(
      Object.entries(value.browsers ?? {}).filter(([, item]) =>
        validView(item),
      ),
    );
    const tagViews = Object.fromEntries(
      Object.entries(value.tagViews ?? {}).filter(
        ([, item]) =>
          validView(item) &&
          "scroll" in item &&
          typeof item.scroll === "number" &&
          Number.isFinite(item.scroll) &&
          item.scroll >= 0,
      ),
    );
    return {
      filesOpen:
        typeof value.filesOpen === "boolean" ? value.filesOpen : undefined,
      sidePath: typeof value.sidePath === "string" ? value.sidePath : undefined,
      tagViews: tagViews as Workspace["tagViews"],
      browsers: browsers as Workspace["browsers"],
      expanded: Array.isArray(value.expanded)
        ? value.expanded.filter((v: unknown) => typeof v === "string")
        : [],
      favorites: (value.favorites ?? []).filter(valid),
      recent: (value.recent ?? []).filter(valid).slice(0, 12),
      tabs: value.tabs.filter(valid).slice(0, 30),
      active: valid(value.active) ? value.active : undefined,
      oldestFirst: Array.isArray(value.oldestFirst)
        ? value.oldestFirst
            .filter((item: unknown) => typeof item === "string")
            .slice(0, 1000)
        : undefined,
      positions: Object.fromEntries(
        Object.entries(value.positions ?? {}).filter(
          ([, position]) =>
            typeof position === "number" &&
            Number.isFinite(position) &&
            position >= 0,
        ),
      ) as Record<string, number>,
    };
  } catch {
    return emptyWorkspace();
  }
}
export function saveWorkspace(root: string, value: Workspace) {
  try {
    localStorage.setItem("quire-workspace:" + root, JSON.stringify(value));
  } catch {
    /* Appearance state must never interrupt note saves. */
  }
}
export type TreeEntry = { path: string; kind: "folder" | "file" };
export type TreeRow = TreeEntry & { name: string; depth: number };
export function folderRows(
  paths: string[],
  entries: TreeEntry[],
  expanded: Set<string>,
): TreeRow[] {
  const folders = new Set(
      entries.filter((e) => e.kind === "folder").map((e) => e.path),
    ),
    files = new Set([
      ...paths,
      ...entries.filter((e) => e.kind === "file").map((e) => e.path),
    ]);
  for (const file of [...files, ...folders]) {
    const parts = file.split("/");
    for (let i = 1; i < parts.length; i++)
      folders.add(parts.slice(0, i).join("/"));
  }
  const children = new Map<string, TreeEntry[]>();
  for (const item of [
    ...Array.from(folders, (path) => ({ path, kind: "folder" as const })),
    ...Array.from(files, (path) => ({ path, kind: "file" as const })),
  ]) {
    const parent = item.path.split("/").slice(0, -1).join("/"),
      group = children.get(parent) ?? [];
    group.push(item);
    children.set(parent, group);
  }
  for (const values of children.values())
    values.sort((a, b) =>
      a.kind === b.kind
        ? a.path.localeCompare(b.path)
        : a.kind === "folder"
          ? -1
          : 1,
    );
  const result: TreeRow[] = [];
  const visit = (parent: string, depth: number) => {
    for (const child of children.get(parent) ?? []) {
      result.push({ ...child, name: child.path.split("/").at(-1)!, depth });
      if (child.kind === "folder" && expanded.has(child.path))
        visit(child.path, depth + 1);
    }
  };
  visit("", 0);
  return result;
}
// A title's namespace is what comes before its last "/", as in Roam:
// "C/Social Proof" is in C, "roam/js/heatmap" in roam/js and so in roam.
// A slash in ordinary text does not make one, so each part before the last
// slash must be a name of letters, digits, spaces and - _ . & ' ( ),
// starting with a letter or digit, not only digits, with no space beside
// a slash. "20/20", "TCP / IP", URLs,
// "tweets: 1/…" and "[[roam/js]]" have none.
const namespacePart = /^[\p{L}\p{N}][\p{L}\p{N} _.&'()-]*$/u;
export function namespaceOf(title: string) {
  const parts = title.split("/"),
    name = parts.pop()!;
  return parts.length &&
    name &&
    name.trimStart() === name &&
    parts.every(
      (part) =>
        namespacePart.test(part) &&
        part.trimEnd() === part &&
        !/^\d+$/.test(part),
    )
    ? parts.join("/")
    : "";
}
// The date a daily page title names: ISO (2026-10-27), ISO with its
// weekday (2026-10-27-Tuesday), Roam (October 27th, 2026), or the open
// notebook's configured title.
// Link targets repeat across a notebook, so parsed dates are kept until
// the daily layout changes.
const parsedDates = new Map<string, string | undefined>();
let parsedLayout = activeDailyLayout();
export function dailyDate(title: string): string | undefined {
  // Every daily title carries a four-digit year; most links have none.
  if (!/\d{4}/.test(title)) return undefined;
  if (parsedLayout !== activeDailyLayout() || parsedDates.size > 100_000) {
    parsedDates.clear();
    parsedLayout = activeDailyLayout();
  }
  if (parsedDates.has(title)) return parsedDates.get(title);
  const date = parseDailyTitle(title);
  parsedDates.set(title, date);
  return date;
}
function parseDailyTitle(title: string): string | undefined {
  const weekday = /^(\d{4}-\d{2}-\d{2})-[A-Za-z]+$/.exec(title);
  if (weekday) return parseDaily(title, "{YYYY}-{MM}-{DD}-{dddd}");
  const { title: pattern } = activeDailyLayout();
  if (pattern !== defaultDailyLayout.title) {
    const configured = parseDaily(title, pattern);
    if (configured) return configured;
  }
  if (/^\d{4}-\d{2}-\d{2}$/.test(title)) {
    const date = new Date(title + "T12:00:00");
    if (
      !Number.isNaN(date.getTime()) &&
      `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, "0")}-${String(date.getDate()).padStart(2, "0")}` ===
        title
    )
      return title;
  }
  if (!/^[A-Za-z]+ \d{1,2}(?:st|nd|rd|th)?, \d{4}$/.test(title)) return;
  const date = new Date(title.replace(/(\d)(st|nd|rd|th)/g, "$1"));
  return Number.isNaN(date.getTime())
    ? undefined
    : `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, "0")}-${String(date.getDate()).padStart(2, "0")}`;
}
