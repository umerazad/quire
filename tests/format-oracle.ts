import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import {
  canonical,
  decodeBatch,
  decodeCheckpoint,
} from "./reference/sync/format.ts";

// This process is a development oracle only, never an application dependency.
const input = JSON.parse(readFileSync(0, "utf8"));
for (const test of input.values) {
  assert.equal(canonical(test.value), test.canonical);
}
for (const test of input.records) {
  const decode = test.kind === "batch" ? decodeBatch : decodeCheckpoint;
  const result = decode(test.name, Uint8Array.from(test.data), {
    notebook: "nb-vectors",
    device: "office",
  });
  if (test.error !== null) {
    assert(!result.ok);
    assert.equal(result.reason, test.error);
  } else {
    assert(result.ok);
    assert.deepEqual(result.value, test.value);
    assert.equal(result.hash, test.hash);
  }
}
