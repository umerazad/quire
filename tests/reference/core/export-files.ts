// exportName and localizeAttachments from rt src/desktop/page-export.ts. The filesystem checks
// in localizeAttachments (realpath inside the notebook, an ordinary file) become `file`, which
// receives the decoded notebook path and answers the checked file or undefined; a decoding
// failure leaves the link unresolved, as the thrown error does in the source.
import { vaultScheme } from "./page-export";

const vaultLink = new RegExp(
  `\\]\\(${vaultScheme.replace(":", "\\:")}([^)\\s]+)\\)`,
  "g",
);
export function localizeAttachments(
  text: string,
  file: (relative: string) => string | undefined,
) {
  const files = new Map<string, string | undefined>();
  for (const [, encoded] of text.matchAll(vaultLink)) {
    if (files.has(encoded)) continue;
    let found: string | undefined;
    try {
      found = file(decodeURIComponent(encoded));
    } catch {
      found = undefined;
    }
    files.set(encoded, found);
  }
  return text.replace(vaultLink, (_, encoded: string) => {
    const found = files.get(encoded);
    if (found) return `](<${found}>)`;
    try {
      return `](${decodeURIComponent(encoded)})`;
    } catch {
      return `](${encoded})`;
    }
  });
}
// A file name for the save dialog from a page title.
export const exportName = (title: string, extension: string) =>
  `${title.replace(/[\\/:*?"<>|\n\r\t]/g, "-").trim() || "Untitled"}.${extension}`;
