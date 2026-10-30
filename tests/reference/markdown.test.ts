import { readFileSync } from "node:fs";
import { expect, it } from "vitest";
import { markdownVector } from "./markdown-oracle";

const vectors = JSON.parse(readFileSync(new URL("../fixtures/markdown/vectors.json", import.meta.url), "utf8"));
for (const vector of vectors) {
  it(`preserves the frozen Markdown behavior: ${vector.name}`, () => {
    expect(markdownVector(vector)).toEqual(vector);
  });
}
