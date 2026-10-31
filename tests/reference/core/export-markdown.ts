// The Markdown functions page-export.ts imports, with each references() call recorded so the
// Rust vectors can answer the same texts with the same references.
import { references as parse, relativeVaultPath, type Reference } from "../markdown/markdown";

export { relativeVaultPath, type Reference };
export const parses = new Map<string, Reference[]>();
export function references(text: string) {
  const found = parse(text);
  parses.set(text, found);
  return found;
}
