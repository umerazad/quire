import assert from "node:assert/strict";
import { execFileSync, spawn } from "node:child_process";
import { once } from "node:events";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createInterface } from "node:readline";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("../", import.meta.url));
execFileSync("cargo", ["build", "-p", "quire_store", "--example", "storage_crash", "--locked"], { cwd: root, stdio: "inherit" });
const metadata = JSON.parse(execFileSync("cargo", ["metadata", "--format-version", "1", "--no-deps", "--locked"], { cwd: root, encoding: "utf8" }));
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
  console.log("Storage crash checks passed: attachment/save/recovery acknowledgments, frozen publication, interrupted transaction.");
} finally {
  rmSync(folder, { recursive: true, force: true });
}
