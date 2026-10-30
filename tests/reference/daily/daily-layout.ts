// Where daily pages live and how they are titled. The layout is stored in
// the notebook (.quire/notebook.json), so every device that opens it agrees.

export type DailyLayout = { folder: string; title: string };
export const defaultDailyLayout: DailyLayout = {
  folder: "",
  title: "{YYYY}-{MM}-{DD}",
};

const months = [
  "January",
  "February",
  "March",
  "April",
  "May",
  "June",
  "July",
  "August",
  "September",
  "October",
  "November",
  "December",
];
const weekdays = [
  "Sunday",
  "Monday",
  "Tuesday",
  "Wednesday",
  "Thursday",
  "Friday",
  "Saturday",
];
const token = /\{(YYYY|MMMM|MM|DD|dddd)\}/g;

const parts = (iso: string) => {
  const [year, month, day] = iso.split("-").map(Number);
  return {
    YYYY: String(year).padStart(4, "0"),
    MM: String(month).padStart(2, "0"),
    MMMM: months[month - 1],
    DD: String(day).padStart(2, "0"),
    dddd: weekdays[new Date(year, month - 1, day, 12).getDay()],
  };
};

export const formatDaily = (pattern: string, iso: string) => {
  const values = parts(iso);
  return pattern.replace(token, (_, name: keyof typeof values) => values[name]);
};

// The ISO date a title written with `pattern` names, or undefined. A title
// whose weekday or month name does not match its date is not a daily title.
export function parseDaily(title: string, pattern: string) {
  const names: string[] = [];
  const source = pattern
    .split(token)
    .map((piece, index) => {
      if (index % 2 === 0) return piece.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
      names.push(piece);
      return piece === "YYYY"
        ? "(\\d{4})"
        : piece === "MM" || piece === "DD"
          ? "(\\d{2})"
          : "([A-Za-z]+)";
    })
    .join("");
  const match = new RegExp(`^${source}$`, "i").exec(title);
  if (!match) return undefined;
  const value = (name: string) =>
    names.includes(name) ? match[names.indexOf(name) + 1] : undefined;
  const year = value("YYYY"),
    monthName = value("MMMM")?.toLowerCase(),
    month =
      value("MM") ??
      (monthName &&
        String(
          months.findIndex((name) => name.toLowerCase() === monthName) + 1,
        ).padStart(2, "0")),
    day = value("DD");
  if (!year || !month || month === "00" || !day) return undefined;
  const iso = `${year}-${month}-${day}`,
    date = new Date(Number(year), Number(month) - 1, Number(day), 12);
  if (
    date.getMonth() !== Number(month) - 1 ||
    date.getDate() !== Number(day) ||
    formatDaily(pattern, iso).toLowerCase() !== title.toLowerCase()
  )
    return undefined;
  return iso;
}

// A reason the layout cannot be used, or undefined when it is valid.
export function layoutProblem(layout: DailyLayout) {
  if (!/\{YYYY\}/.test(layout.title) || !/\{DD\}/.test(layout.title))
    return "The daily title needs {YYYY}, {DD} and {MM} or {MMMM}, such as {YYYY}-{MM}-{DD}.";
  if (!/\{MM\}|\{MMMM\}/.test(layout.title))
    return "The daily title needs {MM} or {MMMM} for the month.";
  if (/[/\\]/.test(layout.title))
    return "The daily title cannot contain a slash. Put folders in the folder pattern.";
  if (
    layout.folder
      .split("/")
      .some((part) => part === ".." || part.startsWith(".")) ||
    layout.folder.startsWith("/") ||
    layout.folder.includes("\\")
  )
    return "The daily folder must be a relative folder inside the notebook, such as Daily/{YYYY}.";
  return undefined;
}

// The layout every caller uses, set when a notebook opens or its settings
// change. Date parsing reads it so links and stamps follow the notebook.
let active = defaultDailyLayout;
export const activeDailyLayout = () => active;
export const setActiveDailyLayout = (layout = defaultDailyLayout) => {
  active = layout;
};

export const dailyTitle = (iso: string, layout = active) =>
  formatDaily(layout.title, iso);
export const dailyFolder = (iso: string, layout = active) =>
  formatDaily(layout.folder, iso).replace(/^\/+|\/+$/g, "");
export const dailyPath = (iso: string, layout = active) => {
  const folder = dailyFolder(iso, layout);
  return `${folder ? `${folder}/` : ""}${dailyTitle(iso, layout)}.md`;
};

const candidates = ["{YYYY}-{MM}-{DD}-{dddd}", "{YYYY}-{MM}-{DD}"];
// The most common folder and title pattern among date-titled pages.
export function detectDailyLayout(paths: string[]): DailyLayout | undefined {
  const counts = new Map<string, number>();
  for (const path of paths) {
    const slash = path.lastIndexOf("/"),
      title = path.slice(slash + 1).replace(/\.md$/i, ""),
      folder = slash < 0 ? "" : path.slice(0, slash);
    for (const pattern of candidates) {
      const iso = parseDaily(title, pattern);
      if (!iso) continue;
      const values = parts(iso);
      const generalized = folder
        .split("/")
        .map((segment) =>
          segment
            .replaceAll(values.YYYY, "{YYYY}")
            .replaceAll(values.MMMM, "{MMMM}")
            .replace(
              new RegExp(`(^|[^\\d])${values.MM}(?=$|[^\\d])`),
              "$1{MM}",
            ),
        )
        .join("/");
      const key = JSON.stringify({ folder: generalized, title: pattern });
      counts.set(key, (counts.get(key) ?? 0) + 1);
      break;
    }
  }
  const best = [...counts].sort((a, b) => b[1] - a[1])[0];
  return best ? (JSON.parse(best[0]) as DailyLayout) : undefined;
}

// Reads .quire/notebook.json. An unknown version keeps the default layout
// and is not overwritten.
export function readNotebookSettings(text?: string): {
  layout: DailyLayout;
  configured: boolean;
  readOnly?: string;
} {
  if (!text) return { layout: defaultDailyLayout, configured: false };
  try {
    const value = JSON.parse(text) as {
      version?: unknown;
      daily?: { folder?: unknown; title?: unknown };
    };
    if (value.version !== 1)
      return {
        layout: defaultDailyLayout,
        configured: false,
        readOnly:
          "This notebook's .quire/notebook.json comes from a newer Quire. Daily notes use the default layout; update Quire to use its settings.",
      };
    const layout = {
      folder: typeof value.daily?.folder === "string" ? value.daily.folder : "",
      title:
        typeof value.daily?.title === "string"
          ? value.daily.title
          : defaultDailyLayout.title,
    };
    const problem = layoutProblem(layout);
    return problem
      ? {
          layout: defaultDailyLayout,
          configured: false,
          readOnly: `.quire/notebook.json has an unusable daily layout: ${problem}`,
        }
      : { layout, configured: !!value.daily };
  } catch {
    return {
      layout: defaultDailyLayout,
      configured: false,
      readOnly:
        ".quire/notebook.json is not valid JSON. Fix or delete it, then reopen the notebook.",
    };
  }
}
export const writeNotebookSettings = (layout: DailyLayout) =>
  `${JSON.stringify({ version: 1, daily: layout }, null, 2)}\n`;
