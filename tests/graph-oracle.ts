import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { addRevision, chainStep, MemoryRevisions } from "./reference/sync/graph.ts";

// Pure graph/chain checks use Bun's TypeScript resolver; no envelope decoding runs here.
const input = JSON.parse(readFileSync(0, "utf8"));
for (const scenario of input.graphs) {
  const store = new MemoryRevisions();
  for (const operation of scenario.operations) {
    const result = addRevision(store, operation.revision);
    assert.equal(result.kind, operation.kind, `graph ${scenario.page}`);
    assert.deepEqual(store.heads(scenario.page), operation.heads);
  }
}
for (const operation of input.chains) {
  const result = chainStep(operation.cursor ?? undefined, operation.batch, operation.hash, (seq) => operation.known?.[seq]);
  assert.equal(result.kind, operation.kind);
}
