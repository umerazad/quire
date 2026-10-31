// rt's side of a page with competing versions, through the frozen reference replica, against a
// real synced folder. `resolve` imports the folder with merges failing, as a merge that cannot be
// stored does, then ends the conflict as rt's `resolveConflict` does (src/desktop/engine/engine.ts
// at 5ea5c22310ff: a save of the chosen text whose base is every head) and publishes. `import`
// imports the folder and reports what this computer then holds. Prints one JSON line.
// A development oracle run by `crates/quire_store/tests/conflict.rs`, never an app dependency.
import { mkdirSync, readFileSync, readdirSync, renameSync, statSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import type { Revision } from "../reference/sync/format";
import { MemoryReplicaStore, Replica, type Folder } from "../reference/sync/replica";

const [root, mode, page, text] = process.argv.slice(2);
if (!root || !page || (mode !== "resolve" && mode !== "import"))
  throw new Error("Usage: bun rt-conflict.ts <folder> resolve|import <page> [text]");

const disk: Folder = {
  async list(directory) {
    try {
      return readdirSync(join(root, directory)).sort();
    } catch {
      return [];
    }
  },
  async read(path) {
    try {
      return new Uint8Array(readFileSync(join(root, path)));
    } catch {
      return undefined;
    }
  },
  async write(path, data) {
    const target = join(root, path);
    mkdirSync(dirname(target), { recursive: true });
    writeFileSync(`${target}.tmp`, data);
    renameSync(`${target}.tmp`, target);
  },
  async exists(path) {
    try {
      statSync(join(root, path));
      return true;
    } catch {
      return false;
    }
  },
};

class RefusingStore extends MemoryReplicaStore {
  refuse = false;
  override put(revision: Revision, heads: string[]) {
    if (this.refuse && revision.parents.length > 1) throw new Error("injected merge failure");
    super.put(revision, heads);
  }
}

const store = new RefusingStore();
const replica = new Replica({ notebook: "nb", device: "rt", folder: disk, store, now: () => 40 });
if (mode === "resolve") {
  store.refuse = true;
  await replica.import().then(
    () => {
      throw new Error("The reference replica merged the competing versions; expected the injected failure.");
    },
    (error) => {
      if (!String(error).includes("injected merge failure")) throw error;
    },
  );
  store.refuse = false;
  const heads = store.heads(page);
  if (heads.length !== 2) throw new Error(`Expected two competing versions, found ${heads.length}.`);
  const saved = replica.save({ page, base: heads, path: replica.head(page)!.path, text: text ?? "" });
  const published = await replica.publish();
  console.log(JSON.stringify({ id: saved.revision.id, competing: heads, heads: store.heads(page), outbox: store.outbox(), published: published.revisions }));
} else {
  const report = await replica.import();
  console.log(JSON.stringify({ heads: store.heads(page), text: replica.head(page)?.text, outbox: store.outbox(), conflicts: [...report.conflicts], problems: report.problems }));
}
