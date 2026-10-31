// The Markdown functions roam.ts imports. references and parseBlocks record each call by text so
// the Rust vectors can answer the same texts with the same parses, and newId counts instead of
// drawing random UUIDs so generated identifiers are reproducible.
import {
  blockText,
  safePagePath,
  pageTitle,
  pageName,
  parseBlocks as parse,
  references as parseReferences,
  type Block,
  type Reference,
} from "../markdown/markdown";

export { blockText, safePagePath, pageTitle, pageName, type Block, type Reference };
export const parses = new Map<string, Reference[]>();
export const blockParses = new Map<string, Block[]>();
let generated = 0;
export function resetRecording(firstId = 1) {
  parses.clear();
  blockParses.clear();
  generated = firstId - 1;
}
export const newId = () => `b-${(++generated).toString(16).padStart(32, "0")}`;
// A copy: roam.ts reverses the result in place, which would reorder the parser's cached array for
// the next call with the same text (an rt defect the port does not reproduce).
export function references(text: string) {
  const found = [...parseReferences(text)];
  parses.set(text, [...found]);
  return found;
}
export function parseBlocks(text: string) {
  const found = parse(text);
  blockParses.set(text, found);
  return found;
}
