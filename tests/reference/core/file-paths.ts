import { safePagePath } from "../markdown/markdown";
export function notebookPath(value: string, markdown = false) {
  const name = value.trim().normalize("NFC"),
    parts = name.split("/");
  if (
    !name ||
    parts.some(
      (part) =>
        !part ||
        part === "." ||
        part === ".." ||
        safePagePath(part).slice(0, -3) !== part,
    )
  )
    throw new Error(
      "This path cannot be used on Windows and macOS. Use ordinary names separated by /, without dots at the beginning or reserved characters.",
    );
  return markdown && !/\.md$/i.test(name) ? name + ".md" : name;
}
export function relativePath(from: string, target: string) {
  const a = from.split("/").slice(0, -1),
    b = target.split("/");
  while (a.length && b.length && a[0] === b[0]) {
    a.shift();
    b.shift();
  }
  return [...a.map(() => ".."), ...b].join("/") || target.split("/").at(-1)!;
}
