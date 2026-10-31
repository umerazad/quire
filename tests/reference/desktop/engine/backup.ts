import { randomUUID } from "node:crypto";
import { promises as fs } from "node:fs";
import path from "node:path";
import { DatabaseSync } from "node:sqlite";
import { snapshotDatabaseSync } from "../notebook-storage";
import { writeFileExclusive } from "../durable-files";
import { decodeBatch, encodeBatch, format, parseName } from "../../sync/format";
import { DiskFolder } from "./disk-folder";
import {
  NotebookEngine,
  notebookFile,
  readNotebookIdentity,
  type NotebookIdentity,
} from "./engine";

// A Quire notebook backup is the synced folder's files plus a copy of
// this computer's database, which holds recovery drafts. Saved changes
// are published first, so the log in the copy is complete.
export async function stageQuireBackup(
  engine: NotebookEngine,
  staging: string,
) {
  await engine.publish();
  if (engine.pendingCount())
    throw new Error(
      "Saved changes could not be published to the synced folder, so a backup would miss them. Check the folder is writable and retry.",
    );
  snapshotDatabaseSync(engine.databaseFile, path.join(staging, "local.sqlite"));
}

// Restores a backed-up Quire notebook into `root` as a new notebook. Its
// change files are rewritten under a new notebook id, so the restored copy
// never mixes with the live one; revision ids, and so page history, are
// unchanged. Recovery drafts from the backed-up database come back too.
export async function restoreQuireBackup(
  backup: string,
  root: string,
  appData: string,
) {
  const source = path.join(backup, "notebook");
  const old = await readNotebookIdentity(source);
  const identity: NotebookIdentity = {
    format,
    id: randomUUID(),
    created: Date.now(),
  };
  await fs.mkdir(root, { recursive: true });
  await writeFileExclusive(
    path.join(root, notebookFile),
    JSON.stringify(identity, null, 2) + "\n",
  );
  const from = new DiskFolder(source),
    to = new DiskFolder(root);
  for (const device of await from.list("devices")) {
    let previous: string | null = null;
    const names = (await from.list(`devices/${device}/log`))
      .map((name) => ({ name, parsed: parseName(name, "qlog") }))
      .filter((entry) => entry.parsed)
      .sort((a, b) => a.parsed!.seq - b.parsed!.seq);
    for (const [index, { name, parsed }] of names.entries()) {
      if (parsed!.seq !== index + 1)
        throw new Error(
          `The backup is missing change ${index + 1} from one computer. Choose another backup.`,
        );
      const data = await from.read(`devices/${device}/log/${name}`);
      const decoded = data
        ? decodeBatch(name, data, { notebook: old.id, device })
        : undefined;
      if (!decoded?.ok)
        throw new Error(
          `A change file in the backup is damaged (${decoded?.reason ?? "unreadable"}). Choose another backup.`,
        );
      const encoded = encodeBatch({
        ...decoded.value,
        notebook: identity.id,
        previous,
      });
      await to.write(`devices/${device}/log/${encoded.name}`, encoded.data);
      previous = encoded.hash;
    }
  }
  // Attachments and imported files keep their paths.
  const copy = async (relative: string) => {
    for (const entry of await fs.readdir(path.join(source, relative), {
      withFileTypes: true,
    })) {
      const child = relative ? `${relative}/${entry.name}` : entry.name;
      if (
        entry.name.startsWith(".") ||
        child === "devices" ||
        child === notebookFile
      )
        continue;
      if (entry.isDirectory()) await copy(child);
      else if (entry.isFile())
        await to.write(
          child,
          new Uint8Array(await fs.readFile(path.join(source, child))),
        );
    }
  };
  await copy("");
  const engine = await NotebookEngine.open({ folder: root, appData });
  try {
    const database = new DatabaseSync(path.join(backup, "local.sqlite"), {
      readOnly: true,
    });
    try {
      for (const draft of database
        .prepare("SELECT path,text,expected,saved_at FROM recovery")
        .all() as {
        path: string;
        text: string;
        expected: string | null;
        saved_at: number;
      }[])
        engine.recover({
          path: draft.path,
          text: draft.text,
          expectedRevision: draft.expected,
          savedAt: Number(draft.saved_at),
        });
    } finally {
      database.close();
    }
  } finally {
    await engine.close();
  }
  return root;
}
