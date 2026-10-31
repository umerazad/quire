import { notebookPath } from "../core/file-paths";
import { promises as fs, constants, existsSync } from "node:fs";
import { createHash, randomUUID } from "node:crypto";
import path from "node:path";
import { DatabaseSync, backup } from "node:sqlite";
import { syncDirectory, publishExclusive } from "./durable-files";
import type { NotebookFiles } from "./notebook-files";
import {
  addRevision,
  copyHistory,
  hasRevision,
  listRevisions,
  openHistory,
  readHistory,
  revisionText,
} from "./history-store";
import {
  notebookStorage,
  migrateLegacyStorage,
  storageHash,
} from "./notebook-storage";
import { serialize, deserialize } from "node:v8";
import { copyWithNewIds, pageName, pageTitle } from "../markdown/markdown";
import type {
  DiskDocument,
  FileEdit,
  Recovery,
  RemoveRequest,
  RevisionInfo,
  SaveResult,
  VaultSnapshot,
} from "../core/types";

export const revision = (text: string) =>
  createHash("sha256").update(text).digest("hex");
const missing = (error: unknown) =>
  (error as NodeJS.ErrnoException).code === "ENOENT";
export class VaultFiles implements NotebookFiles {
  readonly kind = "folder";
  readonly storage: ReturnType<typeof notebookStorage>;
  readonly databasePath: string;
  readonly applicationDataDirectory: string;
  private queues = new Map<string, Promise<unknown>>();
  private durability: DatabaseSync;
  private ready?: Promise<void>;
  private maintenance: Promise<unknown> = Promise.resolve();
  private storageWarnings: string[] = [];
  private seedHistory: boolean;
  constructor(
    public root: string,
    public dataDirectory: string,
  ) {
    this.applicationDataDirectory = dataDirectory;
    this.storage = notebookStorage(root, dataDirectory);
    this.storageWarnings.push(...this.storage.warnings);
    migrateLegacyStorage(root, dataDirectory, this.storage);
    this.seedHistory = !existsSync(this.storage.database);
    this.dataDirectory = this.storage.directory;
    this.databasePath = this.storage.database;
    this.durability = new DatabaseSync(this.databasePath);
    this.durability.exec(`
      PRAGMA journal_mode=WAL;
      PRAGMA synchronous=FULL;
      PRAGMA busy_timeout=5000;
      CREATE TABLE IF NOT EXISTS pending (
        path TEXT PRIMARY KEY,
        text TEXT NOT NULL,
        expected_revision TEXT,
        saved_at INTEGER NOT NULL
      );
      CREATE TABLE IF NOT EXISTS imported_records (fingerprint TEXT PRIMARY KEY);
      CREATE TABLE IF NOT EXISTS imported_sources (source TEXT PRIMARY KEY, fingerprint TEXT NOT NULL);
      CREATE TABLE IF NOT EXISTS foreign_drafts (fingerprint TEXT PRIMARY KEY, device TEXT NOT NULL, path TEXT NOT NULL, text TEXT NOT NULL, expected_revision TEXT, saved_at INTEGER NOT NULL, recovered_path TEXT NOT NULL, recovered_text TEXT NOT NULL);
    `);
    openHistory(this.durability);
  }
  private initialize() {
    this.ready ??= (async () => {
      await syncDirectory(path.dirname(this.dataDirectory));
      await syncDirectory(this.dataDirectory);
      await syncDirectory(this.storage.metadata);
      await syncDirectory(this.storage.devices);
      await this.importCopiedData();
    })().catch((error) => {
      this.ready = undefined;
      throw error;
    });
    return this.ready;
  }
  get searchFile() {
    return this.storage.search;
  }
  close() {
    this.durability.close();
  }
  private async importCopiedData() {
    let recovered = 0;
    const devices = await fs.readdir(this.storage.devices, {
      withFileTypes: true,
    });
    const sources = devices
      .filter(
        (device) =>
          device.name !== this.storage.deviceId &&
          device.isDirectory() &&
          !device.isSymbolicLink(),
      )
      .map((device) => ({
        name: device.name,
        key: `device:${device.name}`,
        database: path.join(
          this.storage.devices,
          device.name,
          "durability.sqlite",
        ),
        journals: path.join(this.storage.devices, device.name, "transactions"),
      }));
    const snapshots = path.join(this.storage.metadata, "history");
    try {
      if ((await fs.lstat(snapshots)).isSymbolicLink())
        throw new Error("Linked history snapshots");
      for (const snapshot of await fs.readdir(snapshots, {
        withFileTypes: true,
      })) {
        const id = snapshot.name.replace(/\.sqlite$/, "");
        if (
          snapshot.isFile() &&
          !snapshot.isSymbolicLink() &&
          snapshot.name.endsWith(".sqlite") &&
          (id !== this.storage.deviceId || this.seedHistory)
        )
          sources.push({
            name: id,
            key: `history:${id}`,
            database: path.join(snapshots, snapshot.name),
            journals: "",
          });
      }
    } catch (error) {
      if (!missing(error))
        this.storageWarnings.push(
          "Portable history snapshots are unavailable. Restore the .quire/history folder before retrying.",
        );
    }
    for (const device of sources) {
      const source = device.database;
      let foreign: DatabaseSync | undefined;
      try {
        const stat = await fs.lstat(source);
        if (stat.isSymbolicLink()) throw new Error("Linked recovery database");
        let wal = "";
        try {
          const value = await fs.stat(source + "-wal");
          wal = `${value.size}:${value.mtimeMs}:${value.ctimeMs}`;
        } catch (error) {
          if (!missing(error)) throw error;
        }
        const journalRoot = device.journals;
        let journalStamp = "";
        if (journalRoot)
          try {
            journalStamp = String((await fs.stat(journalRoot)).mtimeMs);
          } catch (error) {
            if (!missing(error)) throw error;
          }
        let recoveryStamp = "";
        if (journalRoot)
          try {
            recoveryStamp = String(
              (await fs.stat(path.join(path.dirname(journalRoot), "recovery")))
                .mtimeMs,
            );
          } catch (error) {
            if (!missing(error)) throw error;
          }
        const fingerprint = `${stat.size}:${stat.mtimeMs}:${stat.ctimeMs}:${wal}:${journalStamp}:${recoveryStamp}`;
        if (
          this.durability
            .prepare("SELECT fingerprint FROM imported_sources WHERE source=?")
            .get(device.key)?.fingerprint === fingerprint
        )
          continue;
        foreign = new DatabaseSync(source, { readOnly: true });
        if (
          Object.values(
            foreign.prepare("PRAGMA integrity_check").get() ?? {},
          )[0] !== "ok"
        )
          throw new Error("Recovery integrity check failed");
        const histories: Record<string, unknown>[] = readHistory(foreign);
        const drafts = foreign
          .prepare("SELECT path,text,expected_revision,saved_at FROM pending")
          .all();
        if (device.journals) {
          const recovery = path.join(path.dirname(device.journals), "recovery");
          try {
            for (const entry of await fs.readdir(recovery, {
              withFileTypes: true,
            })) {
              if (!entry.name.endsWith(".json")) continue;
              if (!entry.isFile() || entry.isSymbolicLink())
                throw new Error("Invalid copied recovery file");
              const record = JSON.parse(
                await fs.readFile(path.join(recovery, entry.name), "utf8"),
              );
              if (!drafts.some((row) => row.path === record.path))
                drafts.push({
                  path: record.path,
                  text: record.text,
                  expected_revision: record.expectedRevision,
                  saved_at: record.savedAt,
                });
            }
          } catch (error) {
            if (!missing(error)) throw error;
          }
        }
        const foreignDrafts = foreign
          .prepare(
            "SELECT name FROM sqlite_master WHERE type='table' AND name='foreign_drafts'",
          )
          .get();
        const own = readHistory(this.durability);
        if (journalRoot)
          try {
            const journals = await fs.readdir(journalRoot, {
              withFileTypes: true,
            });
            await fs.mkdir(this.journalDirectory(), { recursive: true });
            for (const entry of journals) {
              if (!entry.name.endsWith(".json")) continue;
              if (entry.isSymbolicLink() || !entry.isFile())
                throw new Error("Invalid copied transaction");
              const bytes = await fs.readFile(
                  path.join(journalRoot, entry.name),
                  "utf8",
                ),
                key = "journal:" + revision(bytes);
              if (
                this.durability
                  .prepare("SELECT 1 FROM imported_records WHERE fingerprint=?")
                  .get(key)
              )
                continue;
              const destination = path.join(
                this.journalDirectory(),
                `${revision(bytes)}.json`,
              );
              try {
                await fs.writeFile(destination, bytes, {
                  flag: "wx",
                  mode: 0o600,
                });
                const handle = await fs.open(destination, "r+");
                try {
                  await handle.sync();
                } finally {
                  await handle.close();
                }
              } catch (error) {
                if ((error as NodeJS.ErrnoException).code !== "EEXIST")
                  throw error;
              }
              await syncDirectory(this.journalDirectory());
              this.durability
                .prepare("INSERT OR IGNORE INTO imported_records VALUES (?)")
                .run(key);
            }
          } catch (error) {
            if (!missing(error)) throw error;
          }
        const importRecord = this.durability.prepare(
          "INSERT OR IGNORE INTO imported_records(fingerprint) VALUES (?)",
        );
        this.durability.exec("BEGIN IMMEDIATE");
        try {
          for (const row of own)
            importRecord.run("history:" + revision(JSON.stringify(row)));
          for (const row of histories) {
            if (
              typeof row.path !== "string" ||
              typeof row.text !== "string" ||
              typeof row.revision !== "string" ||
              typeof row.saved_at !== "number"
            )
              throw new Error("Invalid history record");
            if (
              importRecord.run("history:" + revision(JSON.stringify(row)))
                .changes
            )
              addRevision(this.durability, {
                path: row.path,
                text: row.text,
                revision: row.revision,
                saved_at: row.saved_at,
              });
          }
          let deviceRecovered = 0;
          for (const saved of drafts) {
            let row = saved,
              origin = device.name,
              key = "draft:" + revision(JSON.stringify([device.name, saved]));
            const provenance = foreignDrafts
              ? foreign
                  .prepare(
                    "SELECT * FROM foreign_drafts WHERE recovered_path=? AND recovered_text=?",
                  )
                  .get(saved.path, saved.text)
              : undefined;
            if (provenance) {
              row = provenance;
              origin = String(provenance.device);
              key = String(provenance.fingerprint);
            }
            if (
              typeof row.path !== "string" ||
              typeof row.text !== "string" ||
              typeof row.saved_at !== "number" ||
              (row.expected_revision !== null &&
                typeof row.expected_revision !== "string")
            )
              throw new Error("Invalid recovery record");
            if (!importRecord.run(key).changes) continue;
            if (
              this.durability
                .prepare("SELECT text FROM pending WHERE path=?")
                .get(row.path)?.text === row.text
            )
              continue;
            const recoveredPath = `Recovered drafts/Draft ${origin.replace(/[^a-zA-Z0-9_-]/g, "-").slice(0, 8)} ${revision(key).slice(0, 12)}.md`;
            const copied = copyWithNewIds(row.text, recoveredPath, [
              pageName(row.path),
              pageTitle(row.path, row.text),
            ]);
            const metadata =
              /^\uFEFF?---\r?\n([\s\S]*?)\r?\n---(?:\r?\n|$)/.exec(copied);
            const attributes =
              metadata?.[1].replace(
                /^title:/m,
                `recovered-source-title-${revision(key).slice(0, 8)}:`,
              ) ?? "";
            const text = `---\n${attributes ? attributes + "\n" : ""}title: ${JSON.stringify(`Recovered draft: ${pageTitle(row.path, row.text)}`)}\n---\n\n${copied.slice(metadata?.[0].length ?? 0)}`;
            this.durability
              .prepare("INSERT INTO foreign_drafts VALUES (?,?,?,?,?,?,?,?)")
              .run(
                key,
                origin,
                row.path,
                row.text,
                row.expected_revision,
                row.saved_at,
                recoveredPath,
                text,
              );
            this.durability
              .prepare(
                "INSERT INTO pending(path,text,expected_revision,saved_at) VALUES (?,?,NULL,?)",
              )
              .run(recoveredPath, text, row.saved_at);
            deviceRecovered++;
          }
          this.durability
            .prepare("INSERT OR REPLACE INTO imported_sources VALUES (?,?)")
            .run(device.key, fingerprint);
          this.durability.exec("COMMIT");
          recovered += deviceRecovered;
        } catch (error) {
          this.durability.exec("ROLLBACK");
          throw error;
        }
      } catch (error) {
        if (!missing(error))
          this.storageWarnings.push(
            "Another device's recovery data could not be read. Keep its .quire/devices folder, make it available locally and reopen to retry.",
          );
      } finally {
        foreign?.close();
      }
    }
    if (recovered)
      this.storageWarnings.push(
        `Recovered ${recovered} copied-device drafts as separate conflict pages. Review them before saving; original records are retained.`,
      );
  }
  private recoveryPath(relative: string) {
    return path.join(
      this.dataDirectory,
      "recovery",
      `${revision(relative)}.json`,
    );
  }
  private async safePath(relative: string, create = false) {
    if (
      typeof relative !== "string" ||
      path.isAbsolute(relative) ||
      relative.includes("\\") ||
      relative
        .split("/")
        .some((part) => !part || part === ".." || part.startsWith(".")) ||
      !/\.md$/i.test(relative)
    )
      throw new Error(
        "Invalid note path. Choose a Markdown file inside the open notebook.",
      );
    const destination = path.resolve(this.root, relative);
    const parent = path.dirname(destination);
    if (create) {
      let current = this.root;
      for (const part of relative.split("/").slice(0, -1)) {
        const parentDirectory = current;
        current = path.join(current, part);
        let created = false;
        try {
          await fs.mkdir(current);
          created = true;
        } catch (error) {
          if ((error as NodeJS.ErrnoException).code !== "EEXIST") throw error;
        }
        if (created) await syncDirectory(parentDirectory);
        const stat = await fs.lstat(current);
        if (stat.isSymbolicLink() || !stat.isDirectory())
          throw new Error(
            "A note directory is a link or is unavailable. Choose a directory within the notebook.",
          );
      }
    }
    const actualRoot = await fs.realpath(this.root),
      actualParent = await fs.realpath(parent);
    if (
      actualParent !== actualRoot &&
      !actualParent.startsWith(`${actualRoot}${path.sep}`)
    )
      throw new Error(
        "The note resolves outside this notebook. Move it into the notebook before editing.",
      );
    try {
      if ((await fs.lstat(destination)).isSymbolicLink())
        throw new Error(
          "This note is a symbolic link. Open its actual notebook to edit it.",
        );
    } catch (error) {
      if (!missing(error)) throw error;
    }
    return destination;
  }
  async read(relative: string): Promise<DiskDocument> {
    return this.readAt(relative, await this.safePath(relative));
  }
  private async readAt(
    relative: string,
    target: string,
    known?: { size: number; mtimeMs: number },
  ): Promise<DiskDocument> {
    const stat = known ?? (await fs.stat(target));
    if (stat.size > 10 * 1024 * 1024)
      throw new Error(
        `${relative} exceeds the 10 MB note limit. Split it into smaller Markdown files to edit it.`,
      );
    const bytes = await fs.readFile(target);
    const text = new TextDecoder("utf-8", {
      fatal: true,
      ignoreBOM: true,
    }).decode(bytes);
    return {
      path: relative,
      text,
      revision: revision(text),
      modified: stat.mtimeMs,
    };
  }
  private async readIfExists(relative: string) {
    try {
      return await this.read(relative);
    } catch (error) {
      if (missing(error)) return null;
      throw error;
    }
  }
  // Lists notebook folders and files, skipping dot entries, build output and
  // symbolic links, so the Markdown paths it returns stay inside the root.
  private async list() {
    const warnings: string[] = [],
      paths: string[] = [],
      inventory: import("../core/workspace").TreeEntry[] = [];
    const walk = async (folder: string) => {
      let entries;
      try {
        entries = await fs.readdir(path.join(this.root, folder), {
          withFileTypes: true,
        });
      } catch (error) {
        warnings.push(
          `Could not read ${folder || this.root}: ${String(error)}. Make this folder available locally and rescan.`,
        );
        return;
      }
      for (const entry of entries) {
        if (
          entry.name.startsWith(".") ||
          ["node_modules", "dist", "out"].includes(entry.name) ||
          entry.isSymbolicLink()
        )
          continue;
        const relative = path.posix.join(folder, entry.name);
        if (entry.isDirectory()) {
          inventory.push({ path: relative, kind: "folder" });
          await walk(relative);
        } else if (entry.isFile()) {
          inventory.push({ path: relative, kind: "file" });
          if (/\.md$/i.test(entry.name)) paths.push(relative);
        }
      }
    };
    await walk("");
    return { paths, inventory, warnings };
  }
  private static async parallel<T, R>(
    items: T[],
    work: (item: T) => Promise<R>,
    warnings: string[],
  ) {
    const results = Array.from<R | undefined>({ length: items.length });
    let cursor = 0;
    await Promise.all(
      Array.from({ length: Math.min(32, items.length) }, async () => {
        while (cursor < items.length) {
          const index = cursor++;
          try {
            results[index] = await work(items[index]);
          } catch (error) {
            warnings.push(String(error));
          }
        }
      }),
    );
    return results.filter((result): result is R => result !== undefined);
  }
  // Note texts from the last launch, reused when a file's size, modification
  // time and change time are unchanged, so startup reads only changed files.
  // Tools that restore a modification time cannot restore the change time.
  // The monitor rereads reused files after startup to catch anything left, and
  // a stale entry cannot overwrite a note: saves compare the revision on disk.
  private get startupCachePath() {
    return path.join(
      this.applicationDataDirectory,
      "startup-cache",
      `${storageHash(this.root)}.v8`,
    );
  }
  // Paths whose text the last snapshot took from the startup cache.
  reused: string[] = [];
  // Drops the cache after a stale entry, so the next launch reads every file.
  async discardStartupCache() {
    await fs.rm(this.startupCachePath, { force: true });
  }
  private async readStartupCache() {
    const entries = new Map<
      string,
      {
        text: string;
        revision: string;
        modified: number;
        size: number;
        changed: number;
      }
    >();
    try {
      const value = deserialize(await fs.readFile(this.startupCachePath));
      if (value?.version === 2 && value.root === this.root)
        for (const entry of value.documents) entries.set(entry.path, entry);
    } catch (error) {
      if (!missing(error))
        console.warn(
          `Could not read the startup cache: ${String(error)}. Notes are read from disk instead.`,
        );
    }
    return entries;
  }
  private async writeStartupCache(
    documents: DiskDocument[],
    stats: Map<string, { size: number; ctimeMs: number }>,
  ) {
    const target = this.startupCachePath,
      temporary = `${target}.${randomUUID()}.partial`;
    try {
      await fs.mkdir(path.dirname(target), { recursive: true, mode: 0o700 });
      await fs.writeFile(
        temporary,
        serialize({
          version: 2,
          root: this.root,
          documents: documents.map((doc) => ({
            path: doc.path,
            text: doc.text,
            revision: doc.revision,
            modified: doc.modified,
            size: stats.get(doc.path)?.size,
            changed: stats.get(doc.path)?.ctimeMs,
          })),
        }),
        { mode: 0o600 },
      );
      await fs.rename(temporary, target);
      performance.mark("vault:cache-saved");
    } catch (error) {
      await fs.rm(temporary, { force: true }).catch(() => {});
      console.warn(
        `Could not save the startup cache: ${String(error)}. The next launch reads notes from disk.`,
      );
    }
  }
  // Modification times of every Markdown file, for cheap periodic rescans.
  async scan() {
    const { paths, warnings } = await this.list();
    const files = await VaultFiles.parallel(
      paths,
      async (relative) => ({
        path: relative,
        modified: (await fs.stat(path.join(this.root, relative))).mtimeMs,
      }),
      warnings,
    );
    return { files, warnings };
  }
  async snapshot(): Promise<VaultSnapshot> {
    await this.initialize();
    const listing = await this.list(),
      { paths, inventory } = listing,
      warnings = [...this.storageWarnings, ...listing.warnings];
    performance.mark("vault:walked");
    const cached = await this.readStartupCache(),
      stats = new Map<string, { size: number; ctimeMs: number }>(),
      reused: string[] = [];
    const documents = await VaultFiles.parallel(
      paths,
      async (relative) => {
        // The walk already skips dot entries and symbolic links, so the
        // per-file realpath checks in safePath are redundant here.
        const target = path.join(this.root, relative),
          stat = await fs.stat(target),
          hit = cached.get(relative);
        stats.set(relative, stat);
        if (
          hit &&
          hit.modified === stat.mtimeMs &&
          hit.size === stat.size &&
          hit.changed === stat.ctimeMs
        ) {
          reused.push(relative);
          return {
            path: relative,
            text: hit.text,
            revision: hit.revision,
            modified: hit.modified,
          };
        }
        return this.readAt(relative, target, stat);
      },
      warnings,
    );
    this.reused = reused;
    if (reused.length !== cached.size || reused.length !== documents.length)
      setTimeout(
        () => void this.writeStartupCache(documents, stats),
        2000,
      ).unref();
    performance.mark("vault:read");
    const recovery = new Map<string, Recovery>();
    const recoveryFolder = path.dirname(this.recoveryPath("placeholder"));
    try {
      for (const file of await fs.readdir(recoveryFolder)) {
        try {
          const draft = JSON.parse(
            await fs.readFile(path.join(recoveryFolder, file), "utf8"),
          );
          if (typeof draft.path === "string" && typeof draft.text === "string")
            recovery.set(draft.path, draft);
        } catch (error) {
          warnings.push(
            `Could not read a recovery copy: ${String(error)}. Keep ${recoveryFolder} for manual recovery.`,
          );
        }
      }
    } catch (error) {
      if (!missing(error))
        warnings.push(
          `Could not list recovery copies: ${String(error)}. Check local application folder permissions.`,
        );
    }
    const pending = this.durability
      .prepare("SELECT path, text, expected_revision, saved_at FROM pending")
      .all() as {
      path: string;
      text: string;
      expected_revision: string | null;
      saved_at: number;
    }[];
    for (const draft of pending)
      recovery.set(draft.path, {
        path: draft.path,
        text: draft.text,
        expectedRevision: draft.expected_revision,
        savedAt: draft.saved_at,
      });
    return {
      root: this.root,
      name: path.basename(this.root),
      entries: inventory,
      origin: existsSync(path.join(this.storage.metadata, "roam.json"))
        ? "roam"
        : undefined,
      documents: documents.sort((a, b) => a.path.localeCompare(b.path)),
      warnings,
      recovery: [...recovery.values()],
      notebookSettings: await this.readNotebookSettings(),
    };
  }
  private get settingsFile() {
    return path.join(this.storage.metadata, "notebook.json");
  }
  async readNotebookSettings() {
    try {
      return await fs.readFile(this.settingsFile, "utf8");
    } catch (error) {
      if (missing(error)) return undefined;
      throw error;
    }
  }
  // Replaces .quire/notebook.json in one rename, so other devices never
  // read half a file.
  async writeNotebookSettings(text: string) {
    if (typeof text !== "string" || text.length > 64 * 1024)
      throw new Error(
        "Notebook settings are invalid. Reopen Settings and try again.",
      );
    JSON.parse(text);
    const temporary = `${this.settingsFile}.${randomUUID()}.tmp`;
    await fs.writeFile(temporary, text, "utf8");
    try {
      await fs.rename(temporary, this.settingsFile);
    } catch (error) {
      await fs.rm(temporary, { force: true }).catch(() => {});
      throw error;
    }
  }
  private enqueue<T>(key: string, task: () => Promise<T>): Promise<T> {
    const previous = this.queues.get(key) ?? Promise.resolve();
    const gate = this.maintenance;
    const next = Promise.all([previous.catch(() => {}), gate]).then(task);
    this.queues.set(key, next);
    void next
      .finally(() => {
        if (this.queues.get(key) === next) this.queues.delete(key);
      })
      .catch(() => {});
    return next;
  }
  exclusive<T>(task: () => Promise<T>): Promise<T> {
    const pending = [...this.queues.values(), this.maintenance];
    const next = Promise.all(pending.map((item) => item.catch(() => {}))).then(
      task,
    );
    this.maintenance = next.catch(() => {});
    return next;
  }
  async backupDatabase(destination: string) {
    await this.initialize();
    await backup(this.durability, destination);
  }
  async publishHistorySnapshot() {
    await this.initialize();
    const folder = path.join(this.storage.metadata, "history");
    await fs.mkdir(folder, { recursive: true });
    if ((await fs.lstat(folder)).isSymbolicLink())
      throw new Error(
        "History snapshots must stay inside the notebook. Replace the linked history folder and retry.",
      );
    const destination = path.join(folder, `${this.storage.deviceId}.sqlite`),
      temporary = path.join(folder, `.history-${randomUUID()}.partial`);
    try {
      await backup(this.durability, temporary);
      const db = new DatabaseSync(temporary);
      try {
        db.exec("PRAGMA journal_mode=DELETE");
        if (
          Object.values(db.prepare("PRAGMA integrity_check").get() ?? {})[0] !==
          "ok"
        )
          throw new Error(
            "The portable history snapshot failed verification. Keep the notebook and retry backing up.",
          );
      } finally {
        db.close();
      }
      const handle = await fs.open(temporary, "r+");
      try {
        await handle.sync();
      } finally {
        await handle.close();
      }
      await fs.rename(temporary, destination);
      await syncDirectory(folder);
      await syncDirectory(this.storage.metadata);
      return destination;
    } finally {
      await fs.rm(temporary, { force: true });
    }
  }
  recover(draft: Recovery) {
    return this.enqueue(draft.path, () => this.recordRecovery(draft));
  }
  private async recordRecovery(draft: Recovery) {
    await this.initialize();
    this.durability
      .prepare(
        "INSERT INTO pending(path,text,expected_revision,saved_at) VALUES (?,?,?,?) ON CONFLICT(path) DO UPDATE SET text=excluded.text,expected_revision=excluded.expected_revision,saved_at=excluded.saved_at",
      )
      .run(draft.path, draft.text, draft.expectedRevision, draft.savedAt);
  }
  clearRecovery(relative: string, onlyText?: string) {
    return this.enqueue(relative, () =>
      this.removeRecovery(relative, onlyText),
    );
  }
  private async removeRecovery(relative: string, onlyText?: string) {
    await this.initialize();
    const destination = this.recoveryPath(relative);
    if (onlyText !== undefined) {
      let pending = this.durability
        .prepare("SELECT text FROM pending WHERE path=?")
        .get(relative) as { text: string } | undefined;
      if (!pending) {
        try {
          pending = JSON.parse(await fs.readFile(destination, "utf8"));
        } catch (error) {
          if (!missing(error)) throw error;
        }
      }
      if (pending?.text !== onlyText) return;
    }
    await fs.rm(destination, { force: true });
    this.durability.prepare("DELETE FROM pending WHERE path=?").run(relative);
  }
  revisions(relative: string): RevisionInfo[] {
    return listRevisions(this.durability, relative);
  }
  revisionText(relative: string, id: number) {
    const text = revisionText(this.durability, relative, id);
    if (text === undefined)
      throw new Error(
        "That saved revision is unavailable. Refresh page history and choose another version.",
      );
    return text;
  }
  async save(
    relative: string,
    text: string,
    expectedRevision: string | null,
  ): Promise<SaveResult> {
    return this.enqueue(relative, () =>
      this.saveDocument(relative, text, expectedRevision),
    );
  }
  private async saveDocument(
    relative: string,
    text: string,
    expectedRevision: string | null,
  ): Promise<SaveResult> {
    if (typeof text !== "string" || Buffer.byteLength(text) > 10 * 1024 * 1024)
      throw new Error(
        "The note could not be saved because it exceeds 10 MB. Split the note and retry.",
      );
    const destination = await this.safePath(relative, true);
    await this.recordRecovery({
      path: relative,
      text,
      expectedRevision,
      savedAt: Date.now(),
    });
    const disk = await this.readIfExists(relative);
    if ((disk?.revision ?? null) !== expectedRevision)
      return {
        ok: false,
        disk,
        message:
          "This note changed on disk. Your edits are in recovery. Compare the versions or save a separate copy.",
      };
    const temp = path.join(
      path.dirname(destination),
      `.quire-${randomUUID()}.tmp`,
    );
    let handle;
    try {
      const mode = disk ? (await fs.stat(destination)).mode : 0o644;
      handle = await fs.open(temp, "wx", mode);
      await handle.writeFile(text, "utf8");
      await handle.sync();
      await handle.close();
      handle = undefined;
      const latest = await this.readIfExists(relative);
      if ((latest?.revision ?? null) !== expectedRevision)
        return {
          ok: false,
          disk: latest,
          message:
            "Another application changed the note during saving. Compare the versions before retrying.",
        };
      if (disk)
        addRevision(this.durability, {
          path: relative,
          text: disk.text,
          revision: disk.revision,
          saved_at: Date.now(),
        });
      if (!disk) {
        // Hard-link creation fails if an external writer creates this filename first.
        try {
          await publishExclusive(temp, destination);
        } catch (error) {
          if ((error as NodeJS.ErrnoException).code === "EEXIST")
            return {
              ok: false,
              disk: await this.read(relative),
              message:
                "A file with this name was created elsewhere. Choose another name or compare its content.",
            };
          throw error;
        }
      } else await fs.rename(temp, destination);
      await syncDirectory(path.dirname(destination));
      try {
        await this.removeRecovery(relative, text);
      } catch (error) {
        return {
          ok: true,
          revision: revision(text),
          warning: `${relative} was saved, but an old recovery copy could not be removed: ${String(error)}. Check local application folder permissions.`,
        };
      }
      return { ok: true, revision: revision(text) };
    } finally {
      await handle?.close();
      await fs.rm(temp, { force: true });
    }
  }
  remove(request: RemoveRequest, trash: (absolute: string) => Promise<void>) {
    return this.exclusive(async () => {
      const pages = Array.isArray(request?.pages) ? request.pages : [],
        folder = request.folder ? notebookPath(request.folder) : undefined,
        present: string[] = [];
      for (const page of pages) {
        if (folder && !page.path.startsWith(folder + "/"))
          throw new Error(
            `${page.path} is not in ${folder}. Nothing was deleted; try again.`,
          );
        const disk = await this.readIfExists(page.path);
        if (disk && disk.revision !== page.revision)
          throw new Error(
            `${page.path} changed after you chose to delete it. Nothing was deleted; review it and try again.`,
          );
        if (disk) present.push(page.path);
      }
      if (folder) {
        const listed = new Set(pages.map((page) => page.path)),
          unlisted = (await this.list()).paths.filter(
            (item) => item.startsWith(folder + "/") && !listed.has(item),
          );
        if (unlisted.length)
          throw new Error(
            `${unlisted[0]} appeared in ${folder} after you chose to delete it. Nothing was deleted; try again.`,
          );
        await trash(await this.revealPath(folder));
      } else for (const page of present) await trash(await this.safePath(page));
      return (await this.snapshot()).entries ?? [];
    });
  }
  createFolder(relative: string) {
    return this.exclusive(async () => {
      relative = notebookPath(relative);
      await this.safePath(relative + "/Placeholder.md", true);
      return (await this.snapshot()).entries ?? [];
    });
  }
  async revealPath(relative: string) {
    if (!relative) return this.root;
    relative = notebookPath(relative);
    // Resolve both sides: a notebook opened through a link, such as a
    // ~/OneDrive shortcut, has a different real path.
    const [real, target] = await Promise.all([
      fs.realpath(this.root),
      fs.realpath(path.join(this.root, relative)),
    ]);
    if (!target.startsWith(real + path.sep))
      throw new Error(
        "This path is outside the notebook. Choose an item inside it.",
      );
    return target;
  }
  attach(source: string) {
    return this.exclusive(() => this.attachFile(source));
  }
  private async attachFile(source: string) {
    const folder = path.join(this.root, "attachments");
    let created = false;
    try {
      await fs.mkdir(folder);
      created = true;
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code !== "EEXIST") throw error;
    }
    if ((await fs.lstat(folder)).isSymbolicLink())
      throw new Error(
        "The attachments folder is a link. Replace it with a directory inside the notebook and retry.",
      );
    if (created) await syncDirectory(this.root);
    const name = `${randomUUID().slice(0, 8)}-${path.basename(source).replace(/[<>:"/\\|?*]/g, "-")}`;
    const destination = path.join(folder, name);
    await fs.copyFile(source, destination, constants.COPYFILE_EXCL);
    const handle = await fs.open(destination, "r+");
    try {
      await handle.sync();
    } finally {
      await handle.close();
    }
    await syncDirectory(folder);
    return `attachments/${name}`;
  }
  private journalDirectory() {
    return path.join(this.dataDirectory, "transactions");
  }
  commitEdits(edits: FileEdit[]) {
    return this.exclusive(() => this.commitFiles(edits));
  }
  private async commitFiles(edits: FileEdit[]) {
    if (
      !Array.isArray(edits) ||
      !edits.length ||
      edits.length > 1000 ||
      new Set(edits.map((edit) => edit.path)).size !== edits.length
    )
      throw new Error(
        "A change across pages must name each page once and update at most 1000 files. Change fewer pages at a time and retry.",
      );
    edits = edits.map((edit) =>
      edit.remove ? { ...edit, retirement: randomUUID() } : edit,
    );
    if (
      edits.some(
        (edit, index) =>
          edit.remove && edits.slice(index + 1).some((next) => !next.remove),
      )
    )
      throw new Error(
        "Source cleanup must follow destination writes. Retry the operation.",
      );
    const before: (DiskDocument | null)[] = [];
    for (const edit of edits) {
      await this.safePath(edit.path, true);
      const disk = await this.readIfExists(edit.path);
      if ((disk?.revision ?? null) !== edit.expectedRevision)
        throw new Error(
          `${edit.path} changed before the move. Compare its versions and retry moving the block.`,
        );
      before.push(disk);
    }
    await fs.mkdir(this.journalDirectory(), { recursive: true });
    await syncDirectory(this.dataDirectory);
    await syncDirectory(path.dirname(this.journalDirectory()));
    const journal = path.join(this.journalDirectory(), `${randomUUID()}.json`);
    const journalFile = await fs.open(journal, "wx", 0o600);
    try {
      await journalFile.writeFile(
        JSON.stringify({ edits, before, createdAt: Date.now() }),
      );
      await journalFile.sync();
    } finally {
      await journalFile.close();
    }
    await syncDirectory(this.journalDirectory());
    const documents: DiskDocument[] = [];
    for (const edit of edits) {
      await this.applyJournalEdit(edit);
      if (!edit.remove) documents.push(await this.read(edit.path));
    }
    await fs.rm(journal);
    return documents;
  }
  private async applyJournalEdit(edit: FileEdit) {
    const disk = await this.readIfExists(edit.path);
    if (edit.remove) {
      if (!edit.retirement || !/^[a-f0-9-]{36}$/.test(edit.retirement))
        throw new Error(
          "The source cleanup journal is invalid. Retain it for recovery.",
        );
      const folder = path.join(this.dataDirectory, "retired");
      const archived = path.join(folder, edit.retirement + ".md");
      const checkArchive = async () => {
        if (
          (await fs.lstat(folder)).isSymbolicLink() ||
          (await fs.lstat(archived)).isSymbolicLink()
        )
          throw new Error(
            "The recovery archive is linked. Restore ordinary local files before resolving this move.",
          );
        const handle = await fs.open(archived, "r+");
        try {
          const bytes = await handle.readFile();
          await handle.sync();
          if (revision(bytes.toString("utf8")) !== edit.expectedRevision) {
            const text = bytes.toString("utf8");
            if (
              this.durability
                .prepare("SELECT text FROM pending WHERE path=?")
                .get(edit.path)?.text !== text
            )
              this.durability
                .prepare(
                  "INSERT INTO pending(path,text,expected_revision,saved_at) VALUES (?,?,NULL,?) ON CONFLICT(path) DO UPDATE SET text=excluded.text,expected_revision=NULL,saved_at=excluded.saved_at",
                )
                .run(edit.path, text, Date.now());
            throw new Error(
              "The original changed during cleanup. Its newer contents are retained in " +
                archived +
                ". Compare it with the destination before resolving this move.",
            );
          }
        } finally {
          await handle.close();
        }
      };
      if (!disk) {
        try {
          await checkArchive();
        } catch (error) {
          if (!missing(error)) throw error;
        }
        return;
      }
      if (disk.revision !== edit.expectedRevision)
        throw new Error(
          edit.path +
            " changed before source cleanup. Its versions are retained; review them before retrying.",
        );
      await fs.mkdir(folder, { recursive: true });
      if ((await fs.lstat(folder)).isSymbolicLink())
        throw new Error(
          "The recovery archive is linked. Replace it with a local folder before moving files.",
        );
      await syncDirectory(this.dataDirectory);
      try {
        await fs.access(archived);
        throw new Error(
          "An archived source already exists. Keep both files and review the interrupted move.",
        );
      } catch (error) {
        if (!missing(error)) throw error;
      }
      addRevision(this.durability, {
        path: edit.path,
        text: disk.text,
        revision: disk.revision,
        saved_at: Date.now(),
      });
      const source = await this.safePath(edit.path);
      await fs.rename(source, archived);
      await syncDirectory(folder);
      await syncDirectory(path.dirname(source));
      await checkArchive();
      return;
    }
    if (disk?.text !== edit.text) {
      const result = await this.saveDocument(
        edit.path,
        edit.text,
        edit.expectedRevision,
      );
      if (!result.ok)
        throw new Error(
          result.message +
            " The move journal and original versions are retained. Reopen the notebook to retry recovery.",
        );
    }
    if (edit.historyFrom) {
      await this.safePath(edit.historyFrom);
      copyHistory(this.durability, edit.historyFrom, edit.path);
      const old = await this.readIfExists(edit.historyFrom);
      if (old && !hasRevision(this.durability, edit.path, old.revision))
        addRevision(this.durability, {
          path: edit.path,
          text: old.text,
          revision: old.revision,
          saved_at: Date.now(),
        });
    }
  }
  resumeTransactions() {
    return this.exclusive(() => this.resumeFiles());
  }
  private async resumeFiles() {
    await this.initialize();
    const warnings: string[] = [];
    let journals: string[];
    try {
      journals = await fs.readdir(this.journalDirectory());
    } catch (error) {
      if (missing(error)) return warnings;
      throw error;
    }
    for (const name of journals) {
      if (!name.endsWith(".json")) continue;
      const journal = path.join(this.journalDirectory(), name);
      try {
        const { edits } = JSON.parse(await fs.readFile(journal, "utf8")) as {
          edits: FileEdit[];
        };
        if (!Array.isArray(edits) || edits.length < 2 || edits.length > 1000)
          throw new Error("The move journal is malformed.");
        for (const edit of edits) {
          await this.applyJournalEdit(edit);
        }
        await fs.rm(journal);
        warnings.push(
          "Recovered an interrupted block move. Its source and destination are now saved.",
        );
      } catch (error) {
        warnings.push(
          `An interrupted block move needs review: ${String(error)}. Its original versions and pending edits are preserved in ${journal}. Compare those files before retrying the move.`,
        );
      }
    }
    return warnings;
  }
}
