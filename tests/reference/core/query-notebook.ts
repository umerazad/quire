// The part of rt's Notebook (src/core/store.ts) that task-query.ts and projects.ts read.
import type { Task } from "./tasks";

export type Notebook = {
  documents: Map<string, unknown>;
  tasks(): Task[];
  title(path: string): string;
  findPage(target: string): string | undefined;
  taggedBlocks(tag: string): { path: string }[];
  frontTags(path: string): string[];
};
