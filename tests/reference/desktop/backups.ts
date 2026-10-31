import { createHash, randomUUID } from "node:crypto";
import { createReadStream, constants, promises as fs } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { DatabaseSync } from "node:sqlite";
import type { BackupInfo, ProfileId } from "../core/types";
import { VaultFiles, revision } from "./vault";
import type { NotebookFiles } from "./notebook-files";
import { EngineFiles } from "./engine/engine-files";
import { restoreQuireBackup, stageQuireBackup } from "./engine/backup";
import { overlaps } from "./profiles";
import { syncDirectory } from "./durable-files";
import { notebookStorage, snapshotDatabaseSync } from "./notebook-storage";
import { adoptLegacyBackups } from "./legacy-names";

type Entry = { path: string; hash: string; size: number };
const sameFiles = (left: Entry[], right: Entry[]) => {
  const expected = new Map(right.map((file) => [file.path, file]));
  return (
    left.length === right.length &&
    left.every((file) => {
      const other = expected.get(file.path);
      return other?.hash === file.hash && other.size === file.size;
    })
  );
};
type Manifest = {
  version: 1;
  // "quire" backups hold a Quire notebook's synced files and local.sqlite.
  kind?: "quire";
  profile: ProfileId;
  createdAt: number;
  files: Entry[];
  sourceName?: string;
};
const hashFile = async (file: string) => {
  const hash = createHash("sha256");
  for await (const chunk of createReadStream(file)) hash.update(chunk);
  return hash.digest("hex");
};
async function copyFile(source: string, destination: string) {
  await mkdirDurable(path.dirname(destination));
  await fs.copyFile(
    source,
    destination,
    constants.COPYFILE_EXCL | constants.COPYFILE_FICLONE,
  );
  const file = await fs.open(destination, "r+");
  try {
    await file.sync();
  } finally {
    await file.close();
  }
}
async function mkdirDurable(directory: string) {
  const created = await fs.mkdir(directory, { recursive: true, mode: 0o700 });
  if (!created) return;
  const parent = path.dirname(created);
  let current = directory;
  while (true) {
    await syncDirectory(current);
    if (current === parent) break;
    current = path.dirname(current);
  }
}
function checkDatabase(file: string, normalize = false) {
  const db = new DatabaseSync(file, { readOnly: !normalize });
  try {
    if (normalize) db.exec("PRAGMA journal_mode=DELETE");
    const check = db.prepare("PRAGMA integrity_check").get() as Record<
      string,
      string
    >;
    if (Object.values(check)[0] !== "ok")
      throw new Error(
        "The recovery database failed its integrity check. Preserve it and choose another backup.",
      );
  } finally {
    db.close();
  }
}
async function inventory(
  root: string,
  skipGit = false,
  skipDevices = false,
): Promise<Entry[]> {
  const entries: Entry[] = [];
  const walk = async (folder: string) => {
    for (const item of await fs.readdir(path.join(root, folder), {
      withFileTypes: true,
    })) {
      if (skipGit && item.name === ".git") continue;
      const relative = path.posix.join(folder, item.name);
      if (skipDevices && relative === ".quire/devices") continue;
      if (item.isSymbolicLink())
        throw new Error(
          `Backup cannot include symbolic link ${relative}. Replace it with a local file or folder and retry.`,
        );
      if (item.isDirectory()) await walk(relative);
      else if (item.isFile()) {
        const file = path.join(root, relative);
        entries.push({
          path: relative,
          hash: await hashFile(file),
          size: (await fs.stat(file)).size,
        });
      } else
        throw new Error(
          `Backup cannot read ${relative}. Make it an ordinary local file and retry.`,
        );
    }
  };
  await walk("");
  return entries.sort((a, b) => a.path.localeCompare(b.path));
}
async function copyEntries(
  source: string,
  destination: string,
  entries: Entry[],
) {
  await mkdirDurable(destination);
  const directories = new Set([destination]);
  for (const entry of entries) {
    const target = path.join(destination, entry.path);
    await copyFile(path.join(source, entry.path), target);
    if ((await hashFile(target)) !== entry.hash)
      throw new Error(
        "A file changed during backup. Wait for other editors to finish and retry.",
      );
    let folder = path.dirname(target);
    while (folder.startsWith(destination)) {
      directories.add(folder);
      if (folder === destination) break;
      folder = path.dirname(folder);
    }
  }
  for (const folder of [...directories].sort((a, b) => b.length - a.length))
    await syncDirectory(folder);
}
const validPath = (value: unknown): value is string =>
  typeof value === "string" &&
  !!value &&
  !path.isAbsolute(value) &&
  !value.includes("\\") &&
  !value.includes("\0") &&
  value.split("/").every((p) => p && p !== "." && p !== "..");
export class Backups {
  constructor(private dataDirectory: string) {}
  private directory(destination: string, profile: ProfileId, root: string) {
    return path.join(destination, "Quire", profile, revision(root));
  }
  async create(
    files: NotebookFiles,
    destination: string,
    profile: ProfileId,
  ): Promise<BackupInfo> {
    destination = await fs.realpath(destination);
    await adoptLegacyBackups(destination, "Quire");
    if (
      overlaps(destination, await fs.realpath(files.root)) ||
      overlaps(destination, await fs.realpath(this.dataDirectory))
    )
      throw new Error(
        "The backup destination overlaps live notes or app data. Choose a separate backup folder.",
      );
    const staging = await fs.mkdtemp(path.join(tmpdir(), "quire-backup-"));
    const createdAt = Date.now();
    const id = `${new Date(createdAt).toISOString().replace(/[:.]/g, "-")}-${randomUUID().slice(0, 8)}`;
    const parent = this.directory(destination, profile, files.root);
    const partial = path.join(parent, `.partial-${id}`);
    try {
      if (files instanceof EngineFiles) {
        await files.engine.publish();
        const entries = await inventory(files.root, true, true);
        await copyEntries(files.root, path.join(staging, "notebook"), entries);
        await stageQuireBackup(files.engine, staging);
        checkDatabase(path.join(staging, "local.sqlite"));
      } else if (files instanceof VaultFiles)
        await this.stageFolder(files, staging);
      else
        throw new Error(
          "This notebook cannot be backed up by this version of Quire. Update Quire.",
        );
      const entries = await inventory(staging);
      const manifest: Manifest = {
        version: 1,
        ...(files.kind === "quire" ? { kind: "quire" as const } : {}),
        profile,
        createdAt,
        files: entries,
        sourceName: path.basename(files.root),
      };
      await mkdirDurable(parent);
      await copyEntries(staging, partial, entries);
      const manifestFile = path.join(partial, "manifest.json");
      await fs.writeFile(manifestFile, JSON.stringify(manifest), {
        flag: "wx",
        mode: 0o600,
      });
      const handle = await fs.open(manifestFile, "r+");
      try {
        await handle.sync();
      } finally {
        await handle.close();
      }
      await syncDirectory(partial);
      await fs.rename(partial, path.join(parent, id));
      await syncDirectory(parent);
      return { id, createdAt, path: path.join(parent, id) };
    } finally {
      await fs.rm(staging, { recursive: true, force: true });
      await fs.rm(partial, { recursive: true, force: true });
    }
  }
  private async stageFolder(files: VaultFiles, staging: string) {
    await files.exclusive(async () => {
      const entries = await inventory(files.root, true, true);
      await copyEntries(files.root, path.join(staging, "notebook"), entries);
      if (!sameFiles(await inventory(files.root, true, true), entries))
        throw new Error(
          "The notebook changed during backup. Finish changes in other applications and retry.",
        );
      await files.backupDatabase(path.join(staging, "recovery.sqlite"));
      checkDatabase(path.join(staging, "recovery.sqlite"), true);
      for (const device of await fs.readdir(files.storage.devices, {
        withFileTypes: true,
      })) {
        if (device.isSymbolicLink())
          throw new Error(
            "A device storage folder is linked. Replace it with local data before backing up.",
          );
        if (!device.isDirectory()) continue;
        const source = path.join(files.storage.devices, device.name),
          target = path.join(
            staging,
            "notebook",
            ".quire",
            "devices",
            device.name,
          );
        const database = path.join(source, "durability.sqlite");
        try {
          await fs.access(database);
        } catch (error) {
          if ((error as NodeJS.ErrnoException).code === "ENOENT") continue;
          throw error;
        }
        await mkdirDurable(target);
        if (device.name === files.storage.deviceId)
          await copyFile(
            path.join(staging, "recovery.sqlite"),
            path.join(target, "durability.sqlite"),
          );
        else
          snapshotDatabaseSync(
            database,
            path.join(target, "durability.sqlite"),
          );
        for (const kind of ["transactions", "recovery", "retired"]) {
          const folder = path.join(source, kind);
          try {
            await fs.access(folder);
          } catch (error) {
            if ((error as NodeJS.ErrnoException).code === "ENOENT") continue;
            throw error;
          }
          await copyEntries(
            folder,
            path.join(target, kind),
            await inventory(folder),
          );
        }
        await syncDirectory(target);
      }
      for (const kind of ["transactions", "recovery", "retired"]) {
        const source = path.join(files.dataDirectory, kind);
        try {
          await fs.access(source);
        } catch (error) {
          if ((error as NodeJS.ErrnoException).code === "ENOENT") continue;
          throw error;
        }
        await copyEntries(
          source,
          path.join(staging, kind),
          await inventory(source),
        );
      }
    });
  }
  async list(
    destination: string,
    profile: ProfileId,
    root?: string,
  ): Promise<BackupInfo[]> {
    await adoptLegacyBackups(destination, "Quire");
    const parent = root
      ? this.directory(destination, profile, root)
      : path.join(destination, "Quire", profile);
    const result: BackupInfo[] = [];
    let names: string[];
    try {
      names = await fs.readdir(parent);
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code === "ENOENT") return [];
      throw error;
    }
    const folders = root
      ? [parent]
      : names
          .filter((n) => /^[a-f0-9]{64}$/.test(n))
          .map((n) => path.join(parent, n));
    for (const folder of folders) {
      if ((await fs.lstat(folder)).isSymbolicLink()) continue;
      for (const id of (await fs.readdir(folder)).filter(
        (n) => !n.startsWith("."),
      )) {
        try {
          const target = path.join(folder, id);
          if ((await fs.lstat(target)).isSymbolicLink()) continue;
          const manifest = await this.manifest(target, profile);
          result.push({
            id,
            path: target,
            createdAt: manifest.createdAt,
            notebook: manifest.sourceName,
          });
        } catch {
          /* Only complete snapshots are offered. */
        }
      }
    }
    return result.sort((a, b) => b.createdAt - a.createdAt);
  }
  private async manifest(
    directory: string,
    profile: ProfileId,
  ): Promise<Manifest> {
    let value: Manifest;
    try {
      value = JSON.parse(
        await fs.readFile(path.join(directory, "manifest.json"), "utf8"),
      ) as Manifest;
    } catch {
      throw new Error(
        "The backup manifest could not be read. Choose another completed backup.",
      );
    }
    if (
      !value ||
      value.version !== 1 ||
      value.profile !== profile ||
      (value.sourceName !== undefined &&
        typeof value.sourceName !== "string") ||
      !Number.isFinite(value.createdAt) ||
      !Array.isArray(value.files) ||
      (value.kind !== undefined && value.kind !== "quire") ||
      !value.files.some(
        (f) =>
          f?.path ===
          (value.kind === "quire" ? "local.sqlite" : "recovery.sqlite"),
      ) ||
      value.files.some(
        (f) =>
          !f ||
          !validPath(f.path) ||
          !(value.kind === "quire"
            ? f.path === "local.sqlite" || f.path.startsWith("notebook/")
            : f.path === "recovery.sqlite" ||
              /^(notebook|transactions|recovery)\//.test(f.path)) ||
          f.path.split("/").includes(".git") ||
          typeof f.hash !== "string" ||
          !/^[a-f0-9]{64}$/.test(f.hash) ||
          !Number.isSafeInteger(f.size) ||
          f.size < 0,
      ) ||
      new Set(value.files.map((f) => f.path)).size !== value.files.length
    )
      throw new Error(
        "This backup manifest is invalid or belongs to another profile. Choose a complete backup for this profile.",
      );
    return value;
  }
  async restore(
    directory: string,
    parent: string,
    profile: ProfileId,
    validateRoot?: (root: string) => void,
  ): Promise<string> {
    const manifest = await this.manifest(directory, profile);
    const actual = (await inventory(directory)).filter(
      (f) => f.path !== "manifest.json",
    );
    if (!sameFiles(actual, manifest.files))
      throw new Error(
        "Backup verification failed. Files are missing or changed. Choose another completed backup.",
      );
    checkDatabase(
      path.join(
        directory,
        manifest.kind === "quire" ? "local.sqlite" : "recovery.sqlite",
      ),
    );
    parent = await fs.realpath(parent);
    const root = path.join(
      parent,
      `Restored ${new Date().toISOString().replace(/[:.]/g, "-")}-${randomUUID().slice(0, 8)}`,
    );
    validateRoot?.(root);
    if (manifest.kind === "quire")
      return restoreQuireBackup(directory, root, this.dataDirectory);
    await fs.mkdir(root, { mode: 0o700 });
    const entries = manifest.files
      .filter((f) => f.path.startsWith("notebook/"))
      .map((f) => ({ ...f, path: f.path.slice(9) }));
    await copyEntries(path.join(directory, "notebook"), root, entries);
    const storage = notebookStorage(root, this.dataDirectory);
    const metadata = storage.database;
    try {
      await fs.access(metadata);
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error;
      await copyFile(path.join(directory, "recovery.sqlite"), metadata);
    }
    await syncDirectory(path.dirname(metadata));
    for (const kind of ["transactions", "recovery", "retired"]) {
      const entries = manifest.files
        .filter((f) => f.path.startsWith(`${kind}/`))
        .map((f) => ({ ...f, path: f.path.slice(kind.length + 1) }));
      if (entries.length)
        await copyEntries(
          path.join(directory, kind),
          path.join(storage.directory, kind),
          entries,
        );
    }
    await syncDirectory(parent);
    return root;
  }
}
