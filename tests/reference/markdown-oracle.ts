import { editorText, parseBlocks } from "./markdown/markdown";

export function markdownVector(input: { name: string; source: string }) {
  const text = editorText(input.source);
  const blocks = parseBlocks(text);
  const spans = blocks.flatMap((block) => {
    if (block.kind !== "list" && !block.id) return [];
    const id = block.id && /(?:^|[ \t]+)\^[a-zA-Z0-9_-]+[ \t]*$/.exec(text.slice(block.contentFrom, block.contentTo));
    return [{
      marker: block.kind === "list" ? [block.from, block.contentFrom] : [block.contentFrom, block.contentFrom],
      id: id ? [block.contentFrom + id.index, block.contentTo] : null,
    }];
  });
  return JSON.parse(JSON.stringify({ ...input, text, blocks, spans }));
}
