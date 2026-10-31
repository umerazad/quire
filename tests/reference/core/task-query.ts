import type { Notebook } from "./query-notebook";
import { projectMembership, projectPages } from "./projects";
import { isDue, isUpcoming, type Task, type TaskState } from "./tasks";

// A live task list written in a page as {{tasks: QUERY}}. The terms:
// - states: inbox, next, waiting, someday, done, open (not done), due,
//   upcoming (due within a week), all. Several match any of them; with
//   none, open tasks show.
// - #tag, -#tag; [[Page]], -[[Page]] (on the page, or linking it from the
//   task or a parent block); here (the page holding the query);
//   project:Name, -project:Name; words and "quoted words", -word.
// - by:page, by:project, by:state group; sort:due puts the earliest due
//   date first; limit:N caps the list (50 by default).
// Every term other than a state must hold. An empty query means here.
// tests/fixtures/task-queries.json holds cases the terminal shares.
export const taskQueryPattern =
  /\{\{(?:tasks|\[\[tasks\]\])(?::([^}\n]*))?\}\}/;
// A block whose whole text is one task query.
export const taskQueryLine = new RegExp(
  String.raw`^\s*${taskQueryPattern.source}\s*$`,
);

// Splits a query into terms: [[Page Name]] and "quoted words" stay whole,
// with an optional leading "-". The reference filter shares it.
export const queryTokens = (source: string) =>
  source.match(/-?\[\[[^\]]*\]\]|-?"[^"]*"?|\S+/g) ?? [];

type View = TaskState | "open" | "due" | "upcoming" | "all";
const views = new Set<string>([
  "inbox",
  "next",
  "waiting",
  "someday",
  "done",
  "open",
  "due",
  "upcoming",
  "all",
]);
export type TaskGrouping = "page" | "project" | "state";
export type TaskQuery = {
  source: string;
  views: View[];
  tags: string[];
  notTags: string[];
  pages: string[];
  notPages: string[];
  projects: string[];
  notProjects: string[];
  words: string[];
  notWords: string[];
  by?: TaskGrouping;
  sort: "page" | "due";
  limit: number;
};
export type TaskGroup = { label: string; path?: string; tasks: Task[] };
export type TaskQueryResult = { groups: TaskGroup[]; total: number };

export function parseTaskQuery(source: string): TaskQuery | { error: string } {
  const query: TaskQuery = {
    source: source.trim(),
    views: [],
    tags: [],
    notTags: [],
    pages: [],
    notPages: [],
    projects: [],
    notProjects: [],
    words: [],
    notWords: [],
    sort: "page",
    limit: 50,
  };
  const tokens = queryTokens(source);
  if (!tokens.length) query.pages.push("here");
  for (const token of tokens) {
    const negated = token.startsWith("-") && token.length > 1,
      body = negated ? token.slice(1) : token,
      lower = body.toLocaleLowerCase();
    if (body.startsWith('"')) {
      if (body.length < 2 || !body.endsWith('"'))
        return { error: 'A quote is not closed. Add the closing ".' };
      (negated ? query.notWords : query.words).push(
        body.slice(1, -1).toLocaleLowerCase(),
      );
    } else if (body.startsWith("[[") && body.endsWith("]]"))
      (negated ? query.notPages : query.pages).push(body.slice(2, -2).trim());
    else if (body.startsWith("#") && body.length > 1)
      (negated ? query.notTags : query.tags).push(lower);
    else if (lower === "here")
      (negated ? query.notPages : query.pages).push("here");
    else if (!negated && views.has(lower)) query.views.push(lower as View);
    else if (lower.startsWith("project:") && body.length > 8)
      (negated ? query.notProjects : query.projects).push(lower.slice(8));
    else if (!negated && lower.startsWith("by:")) {
      const by = lower.slice(3);
      if (by !== "page" && by !== "project" && by !== "state")
        return {
          error: `Unknown grouping "${by}". Use by:page, by:project or by:state.`,
        };
      query.by = by;
    } else if (!negated && lower.startsWith("sort:")) {
      const sort = lower.slice(5);
      if (sort !== "page" && sort !== "due")
        return { error: `Unknown order "${sort}". Use sort:page or sort:due.` };
      query.sort = sort;
    } else if (!negated && lower.startsWith("limit:")) {
      const limit = Number(lower.slice(6));
      if (!Number.isInteger(limit) || limit < 1)
        return {
          error: `The limit "${lower.slice(6)}" is not a whole number above 0.`,
        };
      query.limit = limit;
    } else if (/^[a-z]+:\S/i.test(body))
      return {
        error: `Unknown term "${body}". Use a state, #tag, [[Page]], project:Name, by:, sort:, limit: or words.`,
      };
    else (negated ? query.notWords : query.words).push(lower);
  }
  return query;
}

const stateLabels: Record<TaskState, string> = {
  inbox: "Inbox",
  next: "Next",
  waiting: "Waiting",
  someday: "Someday",
  done: "Done",
};
const stateOrder: TaskState[] = ["inbox", "next", "waiting", "someday", "done"];

export function runTaskQuery(
  notebook: Notebook,
  query: TaskQuery,
  here: string,
  today: string,
  tasks = notebook.tasks(),
): TaskQueryResult {
  const resolve = (name: string) =>
    name === "here" ? here : notebook.findPage(name);
  const projects = projectPages(notebook),
    projectNamed = (name: string) =>
      projects.filter((path) => {
        const title = notebook.title(path).toLocaleLowerCase();
        return title === name || title.replace(/^projects\//, "") === name;
      }),
    membership =
      query.projects.length ||
      query.notProjects.length ||
      query.by === "project"
        ? projectMembership(notebook, tasks, projects)
        : undefined;
  const touches = (task: Task, path: string | undefined) =>
    !!path &&
    (task.path === path ||
      task.links.some((link) => notebook.findPage(link) === path));
  const hasTag = (task: Task, tag: string) =>
    task.tags.some((own) => own === tag || own.startsWith(tag + "/"));
  const inProject = (task: Task, name: string) =>
    projectNamed(name).some((path) => membership!.get(task.key)?.has(path));
  const inView = (task: Task, view: View) =>
    view === "all" ||
    (view === "open"
      ? task.state !== "done"
      : view === "due"
        ? isDue(task, today)
        : view === "upcoming"
          ? isUpcoming(task, today)
          : task.state === view);
  const views: View[] = query.views.length ? query.views : ["open"];
  const matched = tasks.filter((task) => {
    if (!views.some((view) => inView(task, view))) return false;
    if (!query.tags.every((tag) => hasTag(task, tag))) return false;
    if (query.notTags.some((tag) => hasTag(task, tag))) return false;
    if (!query.pages.every((page) => touches(task, resolve(page))))
      return false;
    if (query.notPages.some((page) => touches(task, resolve(page))))
      return false;
    if (!query.projects.every((name) => inProject(task, name))) return false;
    if (query.notProjects.some((name) => inProject(task, name))) return false;
    const haystack =
      `${task.text} ${notebook.title(task.path)}`.toLocaleLowerCase();
    return (
      query.words.every((word) => haystack.includes(word)) &&
      !query.notWords.some((word) => haystack.includes(word))
    );
  });
  const position = new Map(tasks.map((task, index) => [task.key, index]));
  const byPage = (a: Task, b: Task) =>
    notebook.title(a.path).localeCompare(notebook.title(b.path)) ||
    position.get(a.key)! - position.get(b.key)!;
  matched.sort(
    query.sort === "due"
      ? (a, b) =>
          (a.due ? 0 : 1) - (b.due ? 0 : 1) ||
          (a.due ?? "").localeCompare(b.due ?? "") ||
          byPage(a, b)
      : byPage,
  );
  const total = matched.length,
    shown = matched.slice(0, query.limit);
  if (!query.by) return { groups: [{ label: "", tasks: shown }], total };
  const groups = new Map<string, TaskGroup>();
  const add = (key: string, label: string, task: Task, path?: string) => {
    const group = groups.get(key) ?? { label, path, tasks: [] };
    group.tasks.push(task);
    groups.set(key, group);
  };
  for (const task of shown)
    if (query.by === "page")
      add(task.path, notebook.title(task.path), task, task.path);
    else if (query.by === "state")
      add(task.state, stateLabels[task.state], task);
    else {
      const of = [...(membership!.get(task.key) ?? [])];
      if (!of.length) add("\0none", "No project", task);
      for (const path of of) add(path, notebook.title(path), task, path);
    }
  const order = [...groups.entries()];
  if (query.by === "state")
    order.sort(
      ([a], [b]) =>
        stateOrder.indexOf(a as TaskState) - stateOrder.indexOf(b as TaskState),
    );
  if (query.by === "project")
    order.sort(([a, left], [b, right]) =>
      a === "\0none"
        ? 1
        : b === "\0none"
          ? -1
          : left.label.localeCompare(right.label),
    );
  return { groups: order.map(([, group]) => group), total };
}
