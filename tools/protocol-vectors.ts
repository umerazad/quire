import { writeFileSync } from "node:fs";
import { canonical, revisionId, pageIdForPath, encodeBatch, encodeCheckpoint, parseName, attachmentHash, attachmentFileName, decodeBatch } from "../tests/reference/sync/format.ts";

const values = [null, true, false, 0, -0, 1.0, 1e-7, 1e-6, 1e20, 1e21, 333333333.33333329,
  { "\ue000": 1, "𐀀": 2, "a": ["مرحبا", "میٹنگ", "🙂", "\b\t\n\f\r\u0000", "<>&/\u2028\u2029"] },
  { z: { unknown: { b: 2, a: 1 } }, a: [null, 1e-9] },
];
const fields = { page: "p-example", parents: [], generation: 0, path: "مرحباً.md", deleted: false, text: "- hello\r\n- مرحبا\n- میٹنگ 🙂 ^b-1\n" };
const revision = { ...fields, id: revisionId(fields), created: 1000, device: "office", future: { colour: "jade", weight: 1e-7 } };
const batch = { notebook: "nb-vectors", device: "office", seq: 1, previous: null, created: 1001, revisions: [revision], attachments: ["a".repeat(64) + ".pdf"], future: ["retained", { amount: 1e21 }] };
const encoded = encodeBatch(batch);
writeFileSync(`tests/fixtures/protocol/${encoded.name}`, encoded.data);
const checkpoint = { notebook: batch.notebook, device: batch.device, seq: 1, created: 1002, cursors: { office: { seq: 1, hash: encoded.hash } }, heads: [revision], future: { keep: true } };
const snap = encodeCheckpoint(checkpoint);
writeFileSync(`tests/fixtures/protocol/${snap.name}`, snap.data);
const reparented = { ...fields, parents: ["b".repeat(64), "a".repeat(64)], generation: 2 };
const names = [encoded.name, encoded.name + ".partial", encoded.name + "\n", encoded.name.replace(".qlog", " (1).qlog"), encoded.name.toUpperCase(), "000000000-" + "a".repeat(64) + ".qlog.zst"];
const paths = ["Café.md", "Cafe\u0301.md", "ΟΣ.md", "İ.md", "People/ALICE.md", "میٹنگ.md", "🙂.md"];
const invalid: [string, unknown][] = [
  ["format", { ...batch, format: 2 }],
  ["foreign-notebook", { ...batch, notebook: "other" }],
  ["previous", { ...batch, previous: "a".repeat(64) }],
  ["revision-id", { ...batch, revisions: [{ ...revision, text: "tampered" }] }],
  ["foreign-revision", { ...batch, revisions: [{ ...revision, device: "home" }] }],
  ["attachments", { ...batch, attachments: ["../outside"] }],
];
// encodeBatch deliberately overwrites format; construct future-format bytes directly.
import zlib from "node:zlib";
import { sha256 } from "../tests/reference/sync/format.ts";
const failures = invalid.map(([label, value]) => {
  const text = canonical({ ...value as object, format: label === "format" ? 2 : 1 });
  const name = "000000001-" + sha256(text) + ".qlog.zst";
  const data = zlib.zstdCompressSync(Buffer.from(text));
  writeFileSync(`tests/fixtures/protocol/${label}.zst`, data);
  return { label, name, outcome: decodeBatch(name, data, { notebook: batch.notebook, device: batch.device }) };
});
writeFileSync("tests/fixtures/protocol/vectors.json", JSON.stringify({
  source: "tests/reference/sync/format.ts", values: values.map(value => ({ value, canonical: canonical(value) })),
  revisions: [fields, reparented].map(value => ({ fields: value, id: revisionId(value) })),
  paths: paths.map(path => ({ path, id: pageIdForPath(path) })),
  batch: { value: { ...batch, format: 1 }, name: encoded.name, hash: encoded.hash },
  checkpoint: { value: { ...checkpoint, format: 1 }, name: snap.name, hash: snap.hash },
  names: names.map(name => ({ name, valid: !!parseName(name, "qlog") })),
  attachments: ["a".repeat(64), "a".repeat(64) + ".pdf", "a".repeat(64) + ".PDF", "a".repeat(64) + ".", "../x", "a".repeat(64) + "\n"].map(name => ({ name, hash: attachmentHash(name) ?? null })),
  attachment: { original: "IMAGE.PnG", bytes: [0, 1, 2, 255], name: attachmentFileName(Uint8Array.from([0, 1, 2, 255]), "IMAGE.PnG") }, failures,
}, null, 2) + "\n");

// These are accepted by the source app but exceed the current native JSON model.
// Keep the mismatch visible until the native reader preserves them losslessly.
let deep: unknown = null;
for (let i = 0; i < 140; i++) deep = [deep];
for (const [label, future] of [
  ["unpaired-surrogate", "\ud800"],
  ["deep-json", deep],
] as const) {
  const text = canonical({ ...batch, format: 1, future });
  const data = zlib.zstdCompressSync(Buffer.from(text));
  const name = "000000001-" + sha256(text) + ".qlog.zst";
  const outcome = decodeBatch(name, data, { notebook: batch.notebook, device: batch.device });
  if (!outcome.ok) throw new Error(`Unsupported fixture ${label} must be valid in the source reader.`);
  writeFileSync(`tests/fixtures/protocol/${label}.zst`, data);
  writeFileSync(`tests/fixtures/protocol/${label}.json`, JSON.stringify({ name, sourceAccepts: outcome.ok }) + "\n");
}
