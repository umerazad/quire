import {
  blockText,
  copyWithNewIds,
  pageName,
  parseBlocks,
  references,
  type Block,
} from "../markdown/markdown";
import type { Notebook } from "./template-notebook";
import { localDate } from "../daily/local-date";
import { dailyTitle } from "../daily/daily-layout";
import { dailyDate } from "../daily/workspace";

// Templates live on one page, Templates.md at the notebook root. Each
// top-level block there with children is a template: its text is the
// name, its children are what gets inserted. Typing jj at the start of
// an empty block lists them.
export const templatesTitle = "Templates";
export const isTemplatesPage = (path: string) =>
  path.toLocaleLowerCase() === "templates.md";

export type TemplateDefinition = {
  path: string;
  key: string;
  name: string;
  body: string;
};

export function templatesInPage(
  path: string,
  text: string,
  blocks: Block[],
): TemplateDefinition[] {
  if (!isTemplatesPage(path)) return [];
  return blocks.flatMap((block) => {
    if (block.parent !== undefined) return [];
    const first = blocks.find((child) => child.parent === block.key),
      name = block.text.replace(/[*_`]/g, "").replace(/\s+/g, " ").trim();
    if (!first || !name) return [];
    const line = text.lastIndexOf("\n", first.from - 1) + 1,
      indent = text.slice(line, first.from),
      body = text
        .slice(line, block.subtreeTo)
        .split("\n")
        .map((value) =>
          value.startsWith(indent) ? value.slice(indent.length) : value,
        )
        .join("\n");
    return [{ path, key: block.key, name, body }];
  });
}

const cursorToken = "﷐QUIRE_CURSOR﷑";
const weekdays = [
  "sunday",
  "monday",
  "tuesday",
  "wednesday",
  "thursday",
  "friday",
  "saturday",
];

// A date phrase: today, tomorrow, yesterday, +N or -N days, a weekday
// (the next one after today), or YYYY-MM-DD.
export function templateDate(phrase: string, now: Date) {
  const value = phrase.trim().toLocaleLowerCase(),
    date = new Date(now.getFullYear(), now.getMonth(), now.getDate());
  const shift = (days: number) => {
    date.setDate(date.getDate() + days);
    return localDate(date);
  };
  if (!value || value === "today") return localDate(date);
  if (value === "tomorrow") return shift(1);
  if (value === "yesterday") return shift(-1);
  if (/^[+-]\d{1,4}$/.test(value)) return shift(Number(value));
  const weekday = weekdays.findIndex((day) => day.startsWith(value));
  if (value.length >= 3 && weekday >= 0)
    return shift(((weekday - date.getDay() + 6) % 7) + 1);
  if (/^\d{4}-\d{2}-\d{2}$/.test(value)) return value;
  throw new Error(
    `The template date "${phrase}" is not understood. Use today, tomorrow, yesterday, +3, -1, a weekday or YYYY-MM-DD.`,
  );
}

// Placeholders are single-brace names the guide lists; anything else in
// braces, such as {{tasks: next}}, is copied as written.
const placeholder =
  /(?<!\{)\{(cursor|today|time|page|weekday|date|ask|pick)(?::([^{}\n]*))?\}(?!\})/gi;

// A name used before with a tag, such as a meeting's name.
export type PastName = { name: string; uses: number; last: string };
export type PickRequest = { tag: string; names: PastName[] };
type Ask = (question: string) => string | null | Promise<string | null>;
type Pick = (request: PickRequest) => string | null | Promise<string | null>;

// The names used with a tag: in each block tagged with it, the first page
// link that is not a daily page, else the page's own title when that is
// not a daily page. Most recently used first, then most used. A use's date
// is its daily page's date, else the page's.
export function pastNames(notebook: Notebook, tag: string): PastName[] {
  const names = new Map<string, PastName>();
  for (const match of notebook.taggedBlocks(tag)) {
    const title = notebook.title(match.path),
      link = references(match.snippet).find(
        (ref) => ref.kind === "page" && !dailyDate(ref.target),
      ),
      name =
        link?.target.split(/[#|]/)[0].trim() ?? (dailyDate(title) ? "" : title);
    if (!name || isTemplatesPage(match.path)) continue;
    const key = name.toLocaleLowerCase(),
      last = dailyDate(title) ?? "",
      seen = names.get(key);
    if (!seen) names.set(key, { name, uses: 1, last });
    else {
      seen.uses++;
      if (last > seen.last) seen.last = last;
    }
  }
  return [...names.values()].sort(
    (a, b) =>
      b.last.localeCompare(a.last) ||
      b.uses - a.uses ||
      a.name.localeCompare(b.name),
  );
}

type Fill = {
  notebook: Notebook;
  path: string;
  now: Date;
  prompt: Ask;
  pick?: Pick;
  answers: Map<string, string>;
};

async function fill(text: string, context: Fill) {
  let output = "",
    last = 0;
  for (const match of text.matchAll(placeholder)) {
    const name = match[1].toLocaleLowerCase(),
      argument = (match[2] ?? "").trim();
    let value: string;
    if (name === "cursor") value = cursorToken;
    else if (name === "today")
      value = `[[${dailyTitle(localDate(context.now))}]]`;
    else if (name === "date")
      value = `[[${dailyTitle(templateDate(argument, context.now))}]]`;
    else if (name === "time")
      value = `${String(context.now.getHours()).padStart(2, "0")}:${String(context.now.getMinutes()).padStart(2, "0")}`;
    else if (name === "weekday")
      value = context.now.toLocaleDateString("en-US", { weekday: "long" });
    else if (name === "page") value = context.notebook.title(context.path);
    else {
      const tag = name === "pick" ? `#${argument.replace(/^#/, "")}` : "",
        question = tag || argument || "Value?",
        key = `${name}:${question}`;
      let answer = context.answers.get(key);
      if (answer === undefined) {
        const given =
          tag && context.pick
            ? await context.pick({
                tag,
                names: pastNames(context.notebook, tag),
              })
            : await context.prompt(question);
        if (given === null)
          throw new Error(
            "Template insertion cancelled. The block is unchanged; type jj again to retry.",
          );
        answer = given;
        context.answers.set(key, answer);
      }
      value = answer;
    }
    output += text.slice(last, match.index) + value;
    last = match.index + match[0].length;
  }
  return output + text.slice(last);
}

export async function expandTemplate(
  notebook: Notebook,
  definition: TemplateDefinition,
  path: string,
  options: {
    now?: Date;
    prompt?: Ask;
    pick?: Pick;
  } = {},
) {
  const current = notebook
    .templates()
    .find(
      (value) => value.path === definition.path && value.key === definition.key,
    );
  if (
    !current ||
    current.name !== definition.name ||
    current.body !== definition.body
  )
    throw new Error(
      "The template changed while its list was open. Type jj again and pick its current version.",
    );
  const context: Fill = {
      notebook,
      path,
      now: options.now ?? new Date(),
      prompt: options.prompt ?? (() => null),
      pick: options.pick,
      answers: new Map(),
    },
    body = copyWithNewIds(current.body, path, [
      pageName(current.path),
      notebook.title(current.path),
    ]),
    blocks = parseBlocks(body),
    referenced = new Set(
      references(body)
        .filter((ref) => ref.kind !== "page")
        .map((ref) => ref.target.replace(/^.*#\^/, "")),
    ),
    children = new Map<string | undefined, Block[]>();
  for (const block of blocks)
    children.set(block.parent, [...(children.get(block.parent) ?? []), block]);
  const render = async (block: Block, depth: number): Promise<string> => {
    const indent = "  ".repeat(depth),
      lines = (await fill(blockText(body, block), context)).split("\n"),
      marker =
        block.id && referenced.has(block.id)
          ? `${lines.length > 1 ? `\n${indent}  ` : lines[0] ? " " : ""}^${block.id}`
          : "";
    let output =
      lines
        .map((line, index) => `${indent}${index ? "  " : "- "}${line}`)
        .join("\n") +
      marker +
      "\n";
    for (const child of children.get(block.key) ?? [])
      output += await render(child, depth + 1);
    return output;
  };
  let text = "";
  for (const root of children.get(undefined) ?? [])
    text += await render(root, 0);
  const cursor = text.indexOf(cursorToken);
  text = text.replaceAll(cursorToken, "");
  return { text, cursor: cursor < 0 ? text.trimEnd().length : cursor };
}

// Keeps the block id of the block jj was typed in on the first inserted
// block, so references to it still work.
export function preserveTemplateRoot(text: string, id: string) {
  const first = parseBlocks(text)[0];
  if (!first) return text;
  if (!first.id)
    return (
      text.slice(0, first.contentTo) + ` ^${id}` + text.slice(first.contentTo)
    );
  const old = first.id,
    marker = text.lastIndexOf(`^${old}`, first.contentTo);
  text = text.slice(0, marker) + `^${id}` + text.slice(marker + old.length + 1);
  for (const ref of references(text).reverse()) {
    if (ref.target === old || ref.target.endsWith("#^" + old))
      text =
        text.slice(0, ref.from) +
        text.slice(ref.from, ref.to).replace(old, id) +
        text.slice(ref.to);
  }
  return text;
}
