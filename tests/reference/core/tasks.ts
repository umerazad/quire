import {
  blockText,
  tags,
  references,
  tagExcludedRanges,
  type Block,
  type Reference,
} from "../markdown/markdown";
import type { Tree } from "@lezer/common";
import { dailyDate } from "../daily/workspace";
import { dailyTitle } from "../daily/daily-layout";
export type TaskState = "inbox" | "next" | "waiting" | "someday" | "done";
export type Task = {
  key: string;
  path: string;
  blockKey: string;
  id?: string;
  from: number;
  to: number;
  raw: string;
  text: string;
  marker: string;
  checked: boolean;
  state: TaskState;
  tags: string[];
  links: string[];
  created?: string;
  due?: string;
  completed?: string;
};
export type TaskDateField = "created" | "due" | "completed";
const completionStamp = /[ \t]*✅[ \t]*(\d{4}-\d{2}-\d{2})/u;
// Created and due stamps hold a daily-page link, so the task appears in
// that day's linked references. A plain ISO date also reads.
const linkedStamp = (emoji: string) =>
  new RegExp(
    String.raw`[ \t]*${emoji}[ \t]*(?:\[\[([^\]\n]+)\]\]|(\d{4}-\d{2}-\d{2}))`,
    "u",
  );
const createdStamp = linkedStamp("➕"),
  dueStamp = linkedStamp("📅");
const stampDate = (pattern: RegExp, text: string) => {
  const match = pattern.exec(text);
  return match ? dailyDate(match[1] ?? match[2]) : undefined;
};
// Stamps link the daily page by its configured title, so Obsidian and
// other tools resolve them too.
export const createdStampFor = (date: string) => ` ➕ [[${dailyTitle(date)}]]`;
export const localDate = (date = new Date()) =>
  `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, "0")}-${String(date.getDate()).padStart(2, "0")}`;
export const isDue = (task: Task, today = localDate()) =>
  task.state !== "done" && !!task.due && task.due <= today;
// An ISO date some days after another.
export const addDays = (iso: string, days: number) => {
  const [year, month, day] = iso.split("-").map(Number);
  return localDate(new Date(year, month - 1, day + days, 12));
};
// Open and due after today, within the next week.
export const isUpcoming = (task: Task, today = localDate(), days = 7) =>
  task.state !== "done" &&
  !!task.due &&
  task.due > today &&
  task.due <= addDays(today, days);

// The Tasks filter: words must appear in the task or its page; "-word" and
// "-#tag" hide tasks that contain the word or carry the tag (or a tag
// nested under it).
export function taskFilter(query: string) {
  const include: string[] = [],
    excludeWords: string[] = [],
    excludeTags: string[] = [];
  for (const word of query.trim().toLocaleLowerCase().split(/\s+/)) {
    if (!word) continue;
    if (word.startsWith("-#") && word.length > 2)
      excludeTags.push(word.slice(1));
    else if (word.startsWith("-") && word.length > 1)
      excludeWords.push(word.slice(1));
    else include.push(word);
  }
  const needle = include.join(" ");
  return (task: Task, page: string) => {
    const haystack = `${task.text} ${page}`.toLocaleLowerCase();
    if (needle && !haystack.includes(needle)) return false;
    if (excludeWords.some((word) => haystack.includes(word))) return false;
    const own = task.tags.map((tag) => tag.toLocaleLowerCase());
    return !excludeTags.some((tag) =>
      own.some((item) => item === tag || item.startsWith(tag + "/")),
    );
  };
}
const earliestDate = (targets: Iterable<string>) =>
  [...targets]
    .map((target) => dailyDate(target))
    .filter((value): value is string => !!value)
    .sort()[0];
const marker =
  /^(\[([ xX])\]|\{\{(?:\[\[)?(TODO|DONE)(?:\]\])?\}\})(?:[ \t]+|$)/;
export function readTaskMarker(text: string) {
  const match = marker.exec(text);
  return match
    ? {
        source: match[1],
        prefix: match[0],
        checked: (!!match[2] && match[2] !== " ") || match[3] === "DONE",
      }
    : undefined;
}
export const taskStates: TaskState[] = [
  "inbox",
  "next",
  "waiting",
  "someday",
  "done",
];
// A tag that sets a task's state. #pending, common in Obsidian vaults,
// reads as Waiting.
const isStateTag = (tag: string) =>
  /^#(?:inbox|next|waiting|someday|done|pending)$/i.test(tag);
function taskTags(
  text: string,
  ordinary: ReturnType<typeof tags>,
  refs: Reference[],
  tree?: Tree,
) {
  const wiki = refs.filter(
    (ref) =>
      ref.kind === "page" &&
      text[ref.from - 1] === "#" &&
      (!text[ref.from - 2] || /[\s([{"'*~]/.test(text[ref.from - 2])) &&
      taskStates.includes(ref.target.toLocaleLowerCase() as TaskState),
  );
  if (!wiki.length) return ordinary;
  const ignored = tagExcludedRanges(text, tree, refs);
  return [
    ...ordinary,
    ...wiki
      .filter(
        (ref) =>
          !ignored.some(
            ([start, end]) =>
              !(start >= ref.from && end <= ref.to) &&
              ref.from - 1 < end &&
              ref.to > start,
          ),
      )
      .map((ref) => ({
        from: ref.from - 1,
        to: ref.to,
        text: "#" + ref.target,
      })),
  ].sort((a, b) => a.from - b.from);
}
function within<T extends { from: number; to: number }>(
  items: T[],
  from: number,
  to: number,
) {
  let low = 0,
    high = items.length;
  while (low < high) {
    const middle = (low + high) >>> 1;
    if (items[middle].from < from) low = middle + 1;
    else high = middle;
  }
  const result: T[] = [];
  for (let index = low; index < items.length && items[index].from < to; index++)
    if (items[index].to <= to) result.push(items[index]);
  return result;
}
export function tasksInPage(
  path: string,
  text: string,
  blocks: Block[],
  tokens: ReturnType<typeof tags>,
  refs: Reference[],
  tree?: Tree,
  title?: string,
): Task[] {
  tokens = taskTags(text, tokens, refs, tree);
  const pageDate = title ? dailyDate(title) : undefined;
  const byKey = new Map(blocks.map((block) => [block.key, block])),
    linksByBlock = new Map<string, string[]>();
  const ownLinks = (block: Block) => {
    let links = linksByBlock.get(block.key);
    if (!links) {
      links = within(refs, block.contentFrom, block.contentTo)
        .filter((ref) => ref.kind === "page")
        .map((ref) => ref.target);
      linksByBlock.set(block.key, links);
    }
    return links;
  };
  // A block's date links, without the link that records when it was created.
  const dateLinks = (block: Block) => {
    const created = createdStamp.exec(block.text)?.[1];
    return ownLinks(block).filter((link) => link !== created);
  };
  return blocks.flatMap((block) => {
    if (block.kind !== "list" || !marker.test(block.text)) return [];
    const own = blockText(text, block),
      match = readTaskMarker(own);
    if (!match) return [];
    const ownTags = within(tokens, block.contentFrom, block.contentTo);
    const names = ownTags.map((tag) => tag.text.toLocaleLowerCase()),
      checked = match.checked,
      state: TaskState =
        checked || names.includes("#done")
          ? "done"
          : names.includes("#next")
            ? "next"
            : names.includes("#waiting") || names.includes("#pending")
              ? "waiting"
              : names.includes("#someday")
                ? "someday"
                : "inbox";
    const links = new Set(ownLinks(block)),
      inheritedDates = new Set<string>(),
      visited = new Set([block.key]);
    let parent = block.parent ? byKey.get(block.parent) : undefined;
    while (parent && !visited.has(parent.key)) {
      visited.add(parent.key);
      for (const link of ownLinks(parent)) links.add(link);
      for (const link of dateLinks(parent)) inheritedDates.add(link);
      parent = parent.parent ? byKey.get(parent.parent) : undefined;
    }
    let display = block.text;
    for (const tag of [...ownTags].reverse())
      if (isStateTag(tag.text)) {
        const from = tag.from - block.contentFrom;
        display =
          display.slice(0, from) + display.slice(tag.to - block.contentFrom);
      }
    const body = blockText(text, { ...block, text: display })
      .slice(match.prefix.length)
      .replace(completionStamp, "")
      .replace(createdStamp, "")
      .replace(dueStamp, "")
      .trim();
    return [
      {
        key: `${path}:${block.key}:${block.from}`,
        path,
        blockKey: block.key,
        id: block.id,
        from: block.contentFrom,
        to: block.contentTo,
        raw: block.text,
        text: body || "Untitled action",
        marker: match.source,
        checked,
        state,
        tags: names,
        links: [...links],
        created: stampDate(createdStamp, block.text) ?? pageDate,
        due:
          stampDate(dueStamp, block.text) ??
          earliestDate(dateLinks(block)) ??
          earliestDate(inheritedDates) ??
          pageDate,
        completed: completionStamp.exec(block.text)?.[1],
      },
    ];
  });
}
export function taskChanges(
  task: Task,
  action: TaskState | "toggle",
  today = localDate(),
) {
  const changes: { from: number; to: number; insert: string }[] = [];
  const done = action === "toggle" ? task.state !== "done" : action === "done";
  const nextMarker = task.marker.startsWith("[")
    ? done
      ? "[x]"
      : "[ ]"
    : task.marker.replace(/TODO|DONE/, done ? "DONE" : "TODO");
  if (task.marker !== nextMarker)
    changes.push({
      from: task.from,
      to: task.from + task.marker.length,
      insert: nextMarker,
    });
  const tokens = taskTags(task.raw, tags(task.raw), references(task.raw));
  const remove =
    action === "toggle"
      ? tokens.filter(
          (tag) => !done && tag.text.toLocaleLowerCase() === "#done",
        )
      : done
        ? []
        : tokens.filter((tag) => isStateTag(tag.text));
  for (const tag of remove)
    changes.push({
      from:
        task.from +
        tag.from -
        (/[ \t]/.test(task.raw[tag.from - 1] ?? "") ? 1 : 0),
      to: task.from + tag.to,
      insert: "",
    });
  const end = task.from + task.raw.length,
    stamp = completionStamp.exec(task.raw);
  if (action !== "toggle" && !done)
    changes.push({ from: end, to: end, insert: ` #${action}` });
  if (done && !stamp)
    changes.push({ from: end, to: end, insert: ` ✅ ${today}` });
  if (!done && stamp)
    changes.push({
      from: task.from + stamp.index,
      to: task.from + stamp.index + stamp[0].length,
      insert: "",
    });
  return changes.sort((a, b) => a.from - b.from);
}
const stamps = {
  created: [createdStamp, createdStampFor],
  due: [dueStamp, (date: string) => ` 📅 [[${dailyTitle(date)}]]`],
  completed: [completionStamp, (date: string) => ` ✅ ${date}`],
} as const;
// Sets, replaces or (without a date) removes one of a task's date stamps.
export function taskDateChange(
  task: Task,
  field: TaskDateField,
  date?: string,
) {
  const [pattern, stamp] = stamps[field],
    match = pattern.exec(task.raw),
    insert = date ? stamp(date) : "";
  if (match)
    return [
      {
        from: task.from + match.index,
        to: task.from + match.index + match[0].length,
        insert,
      },
    ];
  const end = task.from + task.raw.length;
  return date ? [{ from: end, to: end, insert }] : [];
}
const months = "Jan Feb Mar Apr May Jun Jul Aug Sep Oct Nov Dec".split(" ");
export const shortDate = (iso: string, today: string) => {
  const [year, month, day] = iso.split("-").map(Number);
  return `${months[month - 1]} ${day}${iso.slice(0, 4) === today.slice(0, 4) ? "" : `, ${year}`}`;
};
export const taskDate = (task: Task, today = localDate()) =>
  task.state === "done"
    ? task.completed && `Done ${shortDate(task.completed, today)}`
    : task.due &&
      (task.due < today
        ? `Overdue · ${shortDate(task.due, today)}`
        : task.due === today
          ? "Today"
          : `Due ${shortDate(task.due, today)}`);
