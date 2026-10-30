import { readFileSync, writeFileSync } from "node:fs";
import { markdownVector } from "../tests/reference/markdown-oracle";

const inputs = JSON.parse(readFileSync("tests/fixtures/markdown/inputs.json", "utf8"));
const vectors = inputs.map(markdownVector);
writeFileSync("tests/fixtures/markdown/vectors.json", JSON.stringify(vectors, null, 2) + "\n");
console.log(`Generated ${vectors.length} Markdown vectors from the copied source parser.`);
