import assert from "node:assert/strict";
import { execFileSync, spawn } from "node:child_process";
import { once } from "node:events";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createInterface } from "node:readline";
import { fileURLToPath } from "node:url";
import { attachmentFileName, encodeBatch, revisionId } from "../tests/reference/sync/format.ts";

const root = fileURLToPath(new URL("../", import.meta.url));
execFileSync("cargo", ["build", "-p", "quire_store", "--example", "storage_crash", "--locked"], { cwd: root, stdio: "inherit" });
const metadata = JSON.parse(execFileSync("cargo", ["metadata", "--format-version", "1", "--no-deps", "--locked"], { cwd: root, encoding: "utf8", maxBuffer: 64 * 2 ** 20 }));
const binary = join(metadata.target_directory, "debug/examples/storage_crash" + (process.platform === "win32" ? ".exe" : ""));
const folder = mkdtempSync(join(tmpdir(), "quire-crash-"));
const database = join(folder, "local.sqlite");

async function killAfterAck(mode: string, version = "1", text = "") {
  const child = spawn(binary, [mode, database, version, text], { stdio: ["pipe", "pipe", "pipe"] });
  let stderr = "";
  child.stderr.on("data", chunk => stderr += chunk);
  const lines = createInterface({ input: child.stdout });
  let timeout: ReturnType<typeof setTimeout>;
  try {
    const [line] = await Promise.race([
      once(lines, "line"),
      once(child, "exit").then(([code, signal]) => { throw new Error(`Probe exited before acknowledgment: ${code}/${signal}: ${stderr}`); }),
      new Promise<never>((_, reject) => { timeout = setTimeout(() => reject(new Error(`Probe timed out: ${stderr}`)), 10_000); }),
    ]);
    const ack = JSON.parse(line);
    assert.equal(ack.mode, mode);
    const exited = once(child, "exit");
    assert(child.kill("SIGKILL"));
    await exited;
    return ack;
  } finally {
    clearTimeout(timeout!);
    lines.close();
    if (child.exitCode === null && child.signalCode === null) {
      const exited = once(child, "exit");
      child.kill("SIGKILL");
      await exited;
    }
  }
}

function inspect() {
  return JSON.parse(execFileSync(binary, ["inspect", database], { encoding: "utf8" }));
}

try {
  const attached = await killAfterAck("attach", "1", "durable attachment bytes");
  assert.match(attached.reference, /^attachments\/[a-f0-9]{64}\.bin$/);
  assert.equal(readFileSync(join(folder, attached.reference), "utf8"), "durable attachment bytes");
  const initialText = `acknowledged مرحبا 🙂 ![](${attached.reference})`;
  const first = await killAfterAck("save", "1", initialText);
  let state = inspect();
  assert.equal(state.text, initialText);
  assert.equal(state.draft, null);
  assert.deepEqual(state.outbox, [first.id]);

  await killAfterAck("draft", "2", "newer recovery میٹنگ");
  state = inspect();
  assert.equal(state.text, initialText);
  assert.equal(state.draft.text, "newer recovery میٹنگ");

  const latest = await killAfterAck("save", "3", "latest acknowledged");
  state = inspect();
  assert.equal(state.text, "latest acknowledged");
  assert.equal(state.draft, null);

  const selected = await killAfterAck("select");
  state = inspect();
  assert.equal(state.pending, selected.name);
  assert(state.outbox.includes(latest.id));

  await killAfterAck("interrupt");
  state = inspect();
  assert.equal(state.text, "latest acknowledged");
  assert.equal(state.pending, selected.name);
  assert(state.outbox.includes(latest.id));
  const notebook = join(folder, "notebook");
  mkdirSync(notebook);
  writeFileSync(join(notebook, "quire-notebook.json"), JSON.stringify({ format: 1, id: "nb-crash", created: 1 }));
  const published = await killAfterAck("publish");
  state = inspect();
  assert.equal(published.name, selected.name);
  assert.deepEqual(state.outbox, []);
  assert.equal(state.pending, null);
  assert.deepEqual(state.cursor, { seq: published.seq, hash: published.hash });
  const log = join(notebook, "devices/device-crash/log", published.name);
  assert(readFileSync(log).length > 0);

  const afterPublication = await killAfterAck("save", "4", "after folder acknowledgment");
  const next = await killAfterAck("publish", "2");
  state = inspect();
  assert.equal(state.text, "after folder acknowledgment");
  assert.equal(next.seq, 2);
  assert.deepEqual(state.outbox, []);
  assert.notEqual(afterPublication.id, latest.id);
  assert(readFileSync(log).length > 0);
  const lateBytes = "late peer attachment";
  const lateName = attachmentFileName(Buffer.from(lateBytes), "probe.bin");
  const fields = { page: "p-process", parents: [], generation: 0, path: "Crash.md", deleted: false, text: `peer update مرحبا 🙂 ![](attachments/${lateName})` };
  const revision = { ...fields, id: revisionId(fields), created: 99, device: "peer-crash" };
  const incoming = encodeBatch({ notebook: "nb-crash", device: "peer-crash", seq: 1, previous: null, created: 99, revisions: [revision], attachments: [lateName] });
  const incomingFile = join(folder, incoming.name);
  writeFileSync(incomingFile, incoming.data);
  const imported = await killAfterAck("import", "1", incomingFile);
  state = inspect();
  assert.equal(imported.applied, 1);
  assert.deepEqual(state.peerCursor, { seq: imported.seq, hash: imported.hash });
  assert.equal(state.peerBatchHash, imported.hash);
  assert.deepEqual(state.pendingAttachments, [lateName]);
  assert.deepEqual(new Map(state.versions.map((version: { id: string; text: string }) => [version.id, version.text])), new Map([[afterPublication.id, "after folder acknowledgment"], [revision.id, fields.text]]));
  assert.deepEqual(state.outbox, []);
  const late = await killAfterAck("attach", "1", lateBytes);
  assert.equal(late.reference, `attachments/${lateName}`);
  state = inspect();
  assert.deepEqual(state.pendingAttachments, []);
  assert.equal(state.peerBatchHash, imported.hash);
  const nextFields = { ...fields, parents: [revision.id], generation: 1, text: "peer folder rescan update" };
  const nextRevision = { ...nextFields, id: revisionId(nextFields), device: "peer-crash", created: 100 };
  const delivered = encodeBatch({ notebook: "nb-crash", device: "peer-crash", seq: 2, previous: incoming.hash, created: 100, revisions: [nextRevision], attachments: [] });
  const peerLog = join(notebook, "devices/peer-crash/log");
  mkdirSync(peerLog, { recursive: true });
  writeFileSync(join(peerLog, delivered.name), delivered.data);
  const rescan = await killAfterAck("rescan");
  assert.equal(rescan.applied, 1);
  assert.equal(rescan.batches, 1);
  state = inspect();
  assert.deepEqual(state.peerCursor, { seq: 2, hash: delivered.hash });
  assert.equal(state.peerBatchHash, imported.hash);
  // The reconciled rescan merges the local head with the delivered peer edit before acknowledging.
  assert.equal(state.versions.length, 1);
  assert.equal(state.versions[0].text, "after folder acknowledgment\npeer folder rescan update");
  console.log("Storage crash checks passed: local/recovery/publication acknowledgments, atomic peer import, late attachment requests, and delivered-folder rescan.");
} finally {
  rmSync(folder, { recursive: true, force: true });
}
