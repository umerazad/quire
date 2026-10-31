// The part of rt's Notebook (src/core/store.ts) that templates.ts reads.
import type { TemplateDefinition } from "./templates";

export type Notebook = {
  templates(): TemplateDefinition[];
  taggedBlocks(tag: string): { path: string; snippet: string }[];
  title(path: string): string;
};
