// The part of rt's Notebook (src/core/store.ts) that page-export.ts reads.
import type { Block } from "../markdown/markdown";

export type Notebook = {
  text(path: string): string;
  title(path: string): string;
  findPage(target: string): string | undefined;
  resolve(target: string): { path: string; block: Block } | { error: string };
};
