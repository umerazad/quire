import type { Notebook } from "./query-notebook";
import type { Task } from "./tasks";

// A front matter tag that marks a project: "project" or "project/<name>".
const projectTag = (tag: string) => /^project(?:\/.+)?$/i.test(tag);

// Project pages: a #project block, a Projects/ title, or a front matter
// tag of project or project/<name>. A #project/<name> tag inside an
// ordinary block does not make its page a project.
export function projectPages(notebook: Notebook) {
  const set = new Set(
    notebook.taggedBlocks("#project").map((match) => match.path),
  );
  for (const path of notebook.documents.keys())
    if (
      /^projects\//i.test(notebook.title(path)) ||
      notebook.frontTags(path).some(projectTag)
    )
      set.add(path);
  return [...set].sort((a, b) =>
    notebook.title(a).localeCompare(notebook.title(b)),
  );
}

// The projects each task belongs to: the project page it sits on, project
// pages it links, and the page whose front matter carries a
// #project/<name> tag the task has.
export function projectMembership(
  notebook: Notebook,
  tasks: Task[],
  projects: string[],
) {
  const set = new Set(projects),
    byTag = new Map<string, string>();
  for (const path of projects)
    for (const tag of notebook.frontTags(path))
      if (/^project\/.+/i.test(tag)) byTag.set(tag.toLocaleLowerCase(), path);
  return new Map(
    tasks.map((task) => [
      task.key,
      new Set([
        ...(set.has(task.path) ? [task.path] : []),
        ...task.links
          .map((link) => notebook.findPage(link))
          .filter((path): path is string => !!path && set.has(path)),
        ...task.tags
          .map((tag) => byTag.get(tag.replace(/^#/, "").toLocaleLowerCase()))
          .filter((path): path is string => !!path),
      ]),
    ]),
  );
}
