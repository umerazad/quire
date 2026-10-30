// Extracted from rt src/core/page-analysis.ts at 5ea5c22310ff (unchanged since 044fb2e79c2b).
// Keeps the link and tagged-block composition; task and template analysis are separate ports.
import { parseMarkdown, parseBlocks, references, tags, frontmatterTags, type Block } from "../markdown/markdown";

export const tagKey = (tag: string) =>
  tag.replace(/^#/, "").normalize("NFC").toLocaleLowerCase();
export function blockAt(blocks: Block[], position: number) {
  let low = 0,
    high = blocks.length - 1;
  while (low <= high) {
    const middle = (low + high) >>> 1;
    if (blocks[middle].from <= position) low = middle + 1;
    else high = middle - 1;
  }
  const block = blocks[high];
  return block && position <= block.to ? block : undefined;
}
export function analyzePage(path: string, text: string) {
  const tree = parseMarkdown(text),
    blocks = parseBlocks(text, tree),
    refs = references(text, tree);
  const links = refs.map((ref) => {
    const block = blockAt(blocks, ref.from);
    return {
      ...ref,
      path,
      text:
        block && ref.from <= block.to
          ? block.text
          : text.slice(ref.from, ref.to),
    };
  });
  const tagTokens = tags(text, tree, refs),
    tagged: { path: string; from: number; blockId?: string; snippet: string; tag: string }[] = [],
    seen = new Set<string>();
  for (const tag of tagTokens) {
    const block = blockAt(blocks, tag.from),
      identity = `${tagKey(tag.text)}:${block?.from ?? tag.from}`;
    if (seen.has(identity)) continue;
    seen.add(identity);
    tagged.push({
      path,
      from: block?.contentFrom ?? tag.from,
      blockId: block?.id,
      snippet: block?.text ?? tag.text,
      tag: tag.text,
    });
  }
  return { blocks, links, tagged, frontTags: frontmatterTags(text) };
}
