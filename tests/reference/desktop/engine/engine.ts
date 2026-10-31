import { promises as fs, mkdirSync, existsSync } from "node:fs";
import path from "node:path";
import { randomUUID } from "node:crypto";
import { notebookPath } from "../../core/file-paths";
import type {
  DiskDocument,
  FileEdit,
  Recovery,
  RemoveRequest,
  RevisionInfo,
  ConflictVersion,
  SaveResult,
  SyncStatus,
  VaultEvent,
  VaultSnapshot,
} from "../../core/types";
import { localDeviceId } from "../notebook-storage";
import { writeFileExclusive } from "../durable-files";
import { format, pageIdForPath, type DeviceStatus } from "../../sync/format";
import { Replica, type ImportReport } from "../../sync/replica";
import { DiskFolder } from "./disk-folder";
import { SqliteReplicaStore, pathKey, type PageRow } from "./sqlite-store";

export const notebookFile = "quire-notebook.json";

// Notebook settings travel in the log as a page that is never listed.
const settingsPage = "s-notebook";
export type NotebookIdentity = { format: number; id: string; created: number };

export async function createQuireNotebook(folder: string) {
  await fs.mkdir(folder, { recursive: true });
  const entries = (await fs.readdir(folder)).filter((n) => !n.startsWith("."));
  if (entries.length)
    throw new Error(
      "Choose an empty folder for a new Quire notebook. The folder you chose already has files in it.",
    );
  const identity: NotebookIdentity = {
    format,
    id: randomUUID(),
    created: Date.now(),
  };
  await writeFileExclusive(
    path.join(folder, notebookFile),
    JSON.stringify(identity, null, 2) + "\n",
  );
  return identity;
}
export async function readNotebookIdentity(
  folder: string,
): Promise<NotebookIdentity> {
  let value: unknown;
  try {
    value = JSON.parse(
      await fs.readFile(path.join(folder, notebookFile), "utf8"),
    );
  } catch {
    throw new Error(
      `${folder} is not a Quire notebook, or its ${notebookFile} has not finished syncing. Wait for the sync tool, or choose the notebook's folder.`,
    );
  }
  const identity = value as NotebookIdentity;
  if (
    !identity ||
    typeof identity.id !== "string" ||
    !/^[0-9a-f-]{36}$/.test(identity.id) ||
    typeof identity.format !== "number"
  )
    throw new Error(
      `${notebookFile} in ${folder} is damaged. Restore it from the sync tool's version history or a backup.`,
    );
  if (identity.format > format)
    throw new Error(
      "This notebook was made by a newer version of Quire. Update Quire to open it.",
    );
  return identity;
}
export const isQuireNotebook = (folder: string) =>
  existsSync(path.join(folder, notebookFile));

// Saves are published after a quiet spell, so a burst of saves makes one
// change file, but never later than the latest. With another Quire app
// open on this computer they go at once, since it reads the folder
// directly; otherwise they wait longer, which writes fewer change files,
// and a sync tool takes seconds between computers anyway.
const publishSoon = { idle: 150, latest: 1_000 },
  publishCalm = { idle: 1_000, latest: 5_000 },
  // An import that hit an unreadable change file tries again this soon; a
  // sync tool may still be writing it.
  retryAfter = 1_000;

export class NotebookEngine {
  readonly replica: Replica;
  private publishTimer?: ReturnType<typeof setTimeout>;
  private publishDeadline?: number;
  private publishing?: Promise<unknown>;
  private syncing?: Promise<ImportReport>;
  private syncAgain = false;
  private retryTimer?: ReturnType<typeof setTimeout>;
  private problems = new Set<string>();
  private nearby = false;
  private listeners = new Set<(event: VaultEvent) => void>();
  private known = new Map<
    string,
    { path: string; head: string; live: boolean }
  >();
  readonly warnings: string[] = [];
  private constructor(
    readonly root: string,
    readonly identity: NotebookIdentity,
    readonly device: string,
    readonly store: SqliteReplicaStore,
    readonly databaseFile: string,
    options: { label: string; app: string; now: () => number },
  ) {
    this.replica = new Replica({
      notebook: identity.id,
      device,
      folder: new DiskFolder(root),
      store,
      now: options.now,
      label: options.label,
      app: options.app,
    });
    for (const page of store.pageVersions()) this.remember(page);
  }

  static async open(options: {
    folder: string;
    appData: string;
    label?: string;
    app?: string;
    now?: () => number;
  }) {
    const root = await fs.realpath(options.folder);
    const identity = await readNotebookIdentity(root);
    const device = localDeviceId(options.appData);
    const directory = path.join(options.appData, "notebooks", identity.id);
    mkdirSync(directory, { recursive: true, mode: 0o700 });
    const databaseFile = path.join(directory, "notebook.sqlite");
    const warnings: string[] = [];
    let store: SqliteReplicaStore;
    try {
      store = new SqliteReplicaStore(databaseFile);
      if (!store.integrity()) {
        store.close();
        throw new Error("integrity check failed");
      }
    } catch (error) {
      // Keep the damaged copy for inspection and rebuild from the folder.
      // Only saves not yet published are lost, at most a few seconds.
      const kept = `${databaseFile}.damaged-${Date.now()}`;
      for (const suffix of ["", "-wal", "-shm"])
        await fs.rename(databaseFile + suffix, kept + suffix).catch(() => {});
      warnings.push(
        `This computer's copy of the notebook was damaged (${String(error)}) and was rebuilt from the synced folder. The damaged copy is kept at ${kept}.`,
      );
      store = new SqliteReplicaStore(databaseFile);
    }
    const engine = new NotebookEngine(
      root,
      identity,
      device,
      store,
      databaseFile,
      {
        label: options.label ?? device,
        app: options.app ?? "",
        now: options.now ?? Date.now,
      },
    );
    engine.warnings.push(...warnings);
    const overlapped = engine.replica.mergeConflicts();
    for (const page of engine.store.pageVersions()) engine.remember(page);
    engine.warnings.push(...engine.overlapWarnings(overlapped));
    if (engine.pendingCount()) engine.schedulePublish();
    await engine.sync();
    engine.settleDrafts();
    // Other apps on this computer see this one is open, and publish at once.
    await engine.replica.writeStatus().catch(() => {});
    return engine;
  }

  private overlapWarnings(pages: Iterable<string>) {
    return [...pages].flatMap((id) => {
      const page = this.store.pageById(id);
      return page ? [overlapWarning(page.path)] : [];
    });
  }

  // Unsaved work left by quitting or a crash is saved from the version it
  // was typed into, which merges it with what changed since. A draft for a
  // page since moved or removed, or one that does not say which version it
  // was typed into, stays for the person to choose.
  private settleDrafts() {
    for (const draft of this.store.recovery()) {
      if (!draft.baseKnown) continue;
      const current = this.store.pageByPath(draft.path);
      const from = draft.expectedRevision
        ? this.store.get(draft.expectedRevision)
        : undefined;
      const here = draft.expectedRevision
        ? !!from && current?.id === from.page
        : !current;
      if (!here) continue;
      try {
        const saved = this.save(draft.path, draft.text, draft.expectedRevision);
        if (saved.ok && saved.warning) this.warnings.push(saved.warning);
      } catch {
        // The draft stays and the editor offers it.
      }
    }
  }

  onChange(listener: (event: VaultEvent) => void) {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }
  private emit(event: VaultEvent) {
    for (const listener of this.listeners) listener(event);
  }
  private remember(page: Pick<PageRow, "id" | "path" | "head" | "deleted">) {
    this.known.set(page.id, {
      path: page.path,
      head: page.head,
      live: !page.deleted,
    });
  }
  private document(page: PageRow): DiskDocument {
    return {
      path: page.path,
      text: page.text,
      revision: page.head,
      modified: page.modified,
    };
  }

  snapshot(): VaultSnapshot {
    const pages = this.store.livePages();
    const folders = new Set<string>();
    for (const page of pages) {
      const parts = page.path.split("/");
      for (let i = 1; i < parts.length; i++)
        folders.add(parts.slice(0, i).join("/"));
    }
    return {
      root: this.root,
      name: path.basename(this.root),
      storage: "quire",
      documents: pages.map((page) => this.document(page)),
      entries: [
        ...[...folders, ...this.emptyFolders()]
          .filter((item, index, all) => all.indexOf(item) === index)
          .sort()
          .map((p) => ({ path: p, kind: "folder" as const })),
        ...pages.map((p) => ({ path: p.path, kind: "file" as const })),
      ],
      notebookSettings: this.readNotebookSettings(),
      warnings: [
        ...this.warnings,
        ...pages
          .filter((page) => page.conflicted)
          .map((page) => conflictWarning(page.path)),
      ],
      recovery: this.store.recovery(),
    };
  }
  readNotebookSettings() {
    return this.store.pageById(settingsPage)?.text;
  }
  writeNotebookSettings(text: string) {
    if (typeof text !== "string" || text.length > 64 * 1024)
      throw new Error(
        "Notebook settings are invalid. Reopen Settings and try again.",
      );
    JSON.parse(text);
    this.replica.save({
      page: settingsPage,
      base: this.store.heads(settingsPage).slice(0, 1),
      path: "notebook-settings",
      text,
    });
    this.schedulePublish();
  }
  // Folders with no pages yet stay on this computer until a page is
  // saved in them.
  private emptyFolders(): string[] {
    try {
      return JSON.parse(this.store.meta("folders") ?? "[]");
    } catch {
      return [];
    }
  }
  createFolder(relative: string) {
    const folder = notebookPath(relative);
    this.store.setMeta(
      "folders",
      JSON.stringify([...new Set([...this.emptyFolders(), folder])].sort()),
    );
  }
  // Deletes pages at the revisions given, all or none, and forgets the
  // folder and the empty folders inside it.
  remove({ pages, folder }: RemoveRequest) {
    const inside = folder ? notebookPath(folder) : undefined;
    if (inside && pages.some((page) => !page.path.startsWith(inside + "/")))
      throw new Error(
        `A page to delete is not in ${inside}. Nothing was deleted; try again.`,
      );
    if (
      inside &&
      this.store
        .livePages()
        .some(
          (page) =>
            page.path.startsWith(inside + "/") &&
            !pages.some((item) => item.path === page.path),
        )
    )
      throw new Error(
        `A page appeared in ${inside} after you chose to delete it. Nothing was deleted; try again.`,
      );
    if (pages.length)
      this.commitEdits(
        pages.map((page) => ({
          path: page.path,
          text: "",
          expectedRevision: page.revision,
          remove: true,
        })),
      );
    if (inside)
      this.store.setMeta(
        "folders",
        JSON.stringify(
          this.emptyFolders().filter(
            (item) => item !== inside && !item.startsWith(inside + "/"),
          ),
        ),
      );
  }
  read(relative: string): DiskDocument | undefined {
    const page = this.store.pageByPath(notebookPath(relative));
    return page ? this.document(page) : undefined;
  }

  // Saves a page edited from `expectedRevision` (null for a new page).
  // When another computer changed the page since then, the edit is saved
  // from the version it was made on and merged with theirs; the merged
  // text comes back so the editor shows it. An edit to a page removed
  // elsewhere brings the page back. Only a version the engine never had,
  // or a name another page has taken since, is refused.
  save(
    relative: string,
    text: string,
    expectedRevision: string | null,
  ): SaveResult {
    if (typeof text !== "string" || Buffer.byteLength(text) > 10 * 2 ** 20)
      throw new Error(
        "The note could not be saved because it exceeds 10 MB. Split the note and retry.",
      );
    const target = notebookPath(relative);
    const result = this.store.transaction(() => {
      const current = this.store.pageByPath(target);
      const from = expectedRevision
        ? this.store.get(expectedRevision)
        : undefined;
      if (
        (expectedRevision && !from) ||
        (current && from && from.page !== current.id)
      )
        return this.checkExpected(target, expectedRevision);
      const saved = this.replica.save({
        page: from?.page ?? current?.id ?? this.newPageId(target),
        base: expectedRevision ? [expectedRevision] : [],
        path: target,
        text,
      });
      this.store.clearRecovery(target, text);
      return saved.overlaps ? overlapWarning(target) : "";
    });
    if (typeof result !== "string") return result!;
    const page = this.store.pageByPath(target);
    // Unchanged text saved from a version deleted elsewhere stays deleted.
    if (!page)
      return {
        ok: false,
        disk: null,
        message:
          "This page was removed on another computer. Your edits are kept in recovery. Save a separate copy to keep them.",
      };
    this.remember(page);
    this.schedulePublish();
    return {
      ok: true,
      revision: page.head,
      ...(page.text !== text ? { text: page.text } : {}),
      ...(result ? { warning: result } : {}),
    };
  }
  // A name used before by a page that has since moved gets a fresh id, so
  // the two pages never share history.
  private newPageId(path: string) {
    const derived = pageIdForPath(path);
    return this.store.pageById(derived) ? `p-${randomUUID()}` : derived;
  }
  private checkExpected(
    target: string,
    expectedRevision: string | null,
  ): SaveResult | undefined {
    const current = this.store.pageByPath(target);
    if ((current?.head ?? null) === expectedRevision) return undefined;
    return {
      ok: false,
      disk: current ? this.document(current) : null,
      message: current
        ? "This page changed on another computer. Your edits are kept in recovery. Compare the versions or save a separate copy."
        : "This page was removed on another computer. Your edits are kept in recovery. Save a separate copy to keep them.",
    };
  }
  private write(edit: {
    path: string;
    text: string;
    expectedRevision: string | null;
    deleted?: boolean;
    page?: string;
  }) {
    const current = this.store.pageByPath(edit.path);
    const page = edit.page ?? current?.id ?? this.newPageId(edit.path);
    const base = edit.page
      ? this.store.heads(edit.page).slice(0, 1)
      : current
        ? [current.head]
        : [];
    return this.replica.save({
      page,
      base,
      path: edit.path,
      text: edit.text,
      deleted: edit.deleted,
    });
  }

  // Applies several page changes as one transaction: all or none. A
  // removal paired with a new path whose `historyFrom` names it is a move,
  // which keeps the page's identity and history.
  commitEdits(edits: FileEdit[]): DiskDocument[] {
    const targets = edits.map((edit) => ({
      ...edit,
      path: notebookPath(edit.path),
      historyFrom: edit.historyFrom
        ? notebookPath(edit.historyFrom)
        : undefined,
    }));
    const moved = new Map<string, string>();
    for (const edit of targets)
      if (
        edit.historyFrom &&
        !edit.remove &&
        targets.some((other) => other.remove && other.path === edit.historyFrom)
      )
        moved.set(edit.historyFrom, edit.path);
    const written = this.store.transaction(() => {
      for (const edit of targets) {
        const problem = this.checkExpected(edit.path, edit.expectedRevision);
        if (problem)
          throw new Error(
            `${edit.path} changed before the change could be applied. Nothing was changed; review the page and try again.`,
          );
      }
      const paths: string[] = [];
      for (const edit of targets) {
        if (edit.remove && moved.has(edit.path)) continue;
        const source =
          edit.historyFrom && moved.get(edit.historyFrom) === edit.path
            ? this.store.pageByPath(edit.historyFrom)
            : undefined;
        if (edit.remove) {
          const current = this.store.pageByPath(edit.path);
          if (current)
            this.write({ ...edit, text: current.text, deleted: true });
        } else {
          this.write({ ...edit, page: source?.id });
          paths.push(edit.path);
        }
      }
      return paths;
    });
    for (const page of this.store.pageVersions()) this.remember(page);
    this.schedulePublish();
    return written.map((p) => this.document(this.store.pageByPath(p)!));
  }

  // The competing versions of a page changed on two computers from the
  // same version, newest first. The first is the one shown.
  async conflictVersions(relative: string): Promise<ConflictVersion[]> {
    const page = this.store.pageByPath(notebookPath(relative));
    if (!page?.conflicted) return [];
    const labels = new Map(
      (await this.statuses()).map((status) => [status.device, status.label]),
    );
    return this.replica.heads(page.id).map((head) => ({
      revision: head.id,
      text: head.text,
      savedAt: head.created,
      computer:
        head.device === this.device
          ? "this computer"
          : (labels.get(head.device) ?? "another computer"),
    }));
  }
  conflicts() {
    return this.store
      .livePages()
      .filter((page) => page.conflicted)
      .map((page) => page.path);
  }
  // Ends a conflict with the chosen text, recorded as following every
  // competing version so no computer sees the conflict again.
  resolveConflict(relative: string, text: string): DiskDocument {
    const page = this.store.pageByPath(notebookPath(relative));
    if (!page?.conflicted)
      throw new Error(
        "This page no longer has competing versions. Reload it to see the current version.",
      );
    this.replica.save({
      page: page.id,
      base: this.store.heads(page.id),
      path: page.path,
      text,
    });
    const resolved = this.store.pageById(page.id)!;
    this.remember(resolved);
    this.schedulePublish();
    return this.document(resolved);
  }
  // Copies a folder of Markdown pages and their files into this notebook.
  // Pages already here are left alone; other files keep their relative
  // paths so links to them still work. The source is only read.
  async importMarkdown(source: string) {
    const root = await fs.realpath(source);
    if (root === this.root || root.startsWith(this.root + path.sep))
      throw new Error("Choose a folder outside this notebook to import.");
    const pages: { path: string; text: string }[] = [];
    const files: string[] = [];
    const walk = async (relative: string) => {
      for (const entry of await fs.readdir(path.join(root, relative), {
        withFileTypes: true,
      })) {
        if (entry.name.startsWith(".")) continue;
        const child = relative ? `${relative}/${entry.name}` : entry.name;
        if (entry.isDirectory()) await walk(child);
        else if (!entry.isFile()) continue;
        else if (/\.md$/i.test(entry.name))
          pages.push({
            path: child,
            text: await fs.readFile(path.join(root, child), "utf8"),
          });
        else files.push(child);
      }
    };
    await walk("");
    const folder = new DiskFolder(this.root);
    for (const file of files)
      await folder.write(
        file,
        new Uint8Array(await fs.readFile(path.join(root, file))),
      );
    let imported = 0;
    for (let start = 0; start < pages.length; start += 500)
      this.store.transaction(() => {
        for (const page of pages.slice(start, start + 500)) {
          const target = notebookPath(page.path);
          if (this.store.pageByPath(target)) continue;
          this.write({ path: target, text: page.text, expectedRevision: null });
          imported++;
        }
      });
    for (const page of this.store.pageVersions()) this.remember(page);
    this.schedulePublish();
    return {
      pages: imported,
      skipped: pages.length - imported,
      files: files.length,
    };
  }
  recover(draft: Recovery) {
    this.store.recordRecovery({ ...draft, path: notebookPath(draft.path) });
  }
  clearRecovery(relative: string, onlyText?: string) {
    this.store.clearRecovery(notebookPath(relative), onlyText);
  }

  revisions(relative: string): RevisionInfo[] {
    const page = this.store.pageByPath(notebookPath(relative));
    if (!page) return [];
    return this.store
      .revisionsOf(page.id)
      .filter((row) => row.id !== page.head)
      .map((row) => ({
        id: row.rowid,
        revision: row.id,
        savedAt: row.created,
      }));
  }
  revisionText(relative: string, id: number) {
    const page = this.store.pageByPath(notebookPath(relative));
    const revision = this.store.revisionByRow(id);
    if (!page || !revision || revision.page !== page.id)
      throw new Error(
        "That version of the page is not available. Reopen page history and choose another.",
      );
    return revision.text;
  }

  private schedulePublish() {
    const now = Date.now(),
      { idle, latest } = this.nearby ? publishSoon : publishCalm;
    this.publishDeadline ??= now + latest;
    clearTimeout(this.publishTimer);
    this.publishTimer = setTimeout(
      () => void this.publish().catch(() => {}),
      Math.max(0, Math.min(idle, this.publishDeadline - now)),
    );
  }
  // Publishes every saved change. Concurrent calls share one run.
  async publish() {
    clearTimeout(this.publishTimer);
    this.publishDeadline = undefined;
    while (this.publishing) await this.publishing.catch(() => {});
    const run = (async () => {
      while (this.store.outbox().length) await this.replica.publish();
      await this.replica.writeStatus();
    })();
    this.publishing = run;
    try {
      await run;
    } finally {
      if (this.publishing === run) this.publishing = undefined;
    }
  }
  pendingCount() {
    return this.store.outbox().length;
  }
  statuses(): Promise<DeviceStatus[]> {
    return this.replica.statuses();
  }
  async syncStatus(): Promise<SyncStatus> {
    return {
      pending: this.pendingCount(),
      computers: (await this.statuses())
        .filter((status) => status.device !== this.device)
        .map((status) => ({
          label: status.label,
          seen: status.heartbeat,
          waiting: status.outbox,
        }))
        .sort((a, b) => a.label.localeCompare(b.label)),
    };
  }

  // Imports changes from other computers and reports changed pages. A
  // request that arrives during an import runs another once it ends: the
  // running one may have listed the folder before the new file landed.
  sync(): Promise<ImportReport> {
    if (this.syncing) {
      this.syncAgain = true;
      return this.syncing;
    }
    const run = (async () => {
      let report: ImportReport | undefined;
      do {
        this.syncAgain = false;
        const next = await this.importOnce();
        report = report ? combine(report, next) : next;
      } while (this.syncAgain);
      for (const id of report.conflicts)
        if (this.store.heads(id).length < 2) report.conflicts.delete(id);
      return report;
    })().finally(() => {
      this.syncing = undefined;
    });
    this.syncing = run;
    return run;
  }
  private async importOnce() {
    clearTimeout(this.retryTimer);
    const report = await this.replica.import();
    this.nearby = this.replica.nearby(await this.statuses());
    for (const id of this.separateSamePaths()) report.pages.add(id);
    // Removals go first, so a name freed by one page and taken by another
    // ends up showing the new page.
    const changed = [...report.pages]
      .filter((id) => id !== settingsPage)
      .map((id) => ({
        before: this.known.get(id),
        page: this.store.pageById(id),
      }))
      .filter((change) => change.page);
    const live = new Set(
      changed.filter((c) => !c.page!.deleted).map((c) => pathKey(c.page!.path)),
    );
    for (const { before, page } of changed)
      if (
        before?.live &&
        (page!.deleted || before.path !== page!.path) &&
        !live.has(pathKey(before.path))
      )
        this.emit({ type: "delete", path: before.path });
    for (const { page } of changed) {
      if (!page!.deleted)
        this.emit({ type: "upsert", document: this.document(page!) });
      if (page!.conflicted)
        this.emit({ type: "warning", message: conflictWarning(page!.path) });
      this.remember(page!);
    }
    for (const message of this.overlapWarnings(report.overlapped))
      this.emit({ type: "warning", message });
    if (this.pendingCount()) this.schedulePublish();
    // A problem is reported once it survives a retry, so a file caught
    // mid-delivery is read again quietly.
    const seen = this.problems;
    this.problems = new Set();
    for (const problem of report.problems) {
      const key = `${problem.device}\n${problem.file ?? ""}\n${problem.reason}`;
      this.problems.add(key);
      if (seen.has(key))
        this.emit({
          type: "warning",
          message: `Changes from another computer could not be read${problem.file ? ` (${problem.file})` : ""}: ${problem.reason}. Quire will try again.`,
        });
    }
    if (report.problems.length)
      this.retryTimer = setTimeout(
        () => void this.sync().catch(() => {}),
        retryAfter,
      );
    return report;
  }

  // Two pages can arrive with one name, for example when one computer
  // renames a page to a name another computer just used. The page with the
  // larger id moves to the first free "Name (2).md". Every computer makes
  // the same choice, so they agree without a conflict.
  private separateSamePaths() {
    const byKey = new Map<string, PageRow[]>();
    for (const page of this.store.livePages()) {
      const key = pathKey(page.path);
      byKey.set(key, [...(byKey.get(key) ?? []), page]);
    }
    const moved: string[] = [];
    if ([...byKey.values()].every((group) => group.length < 2)) return moved;
    this.store.transaction(() => {
      for (const group of byKey.values()) {
        if (group.length < 2) continue;
        for (const page of [...group]
          .sort((a, b) => (a.id < b.id ? -1 : 1))
          .slice(1)) {
          const stem = page.path.replace(/\.md$/i, "");
          let n = 2;
          while (byKey.has(pathKey(`${stem} (${n}).md`))) n++;
          const target = `${stem} (${n}).md`;
          byKey.set(pathKey(target), [page]);
          this.replica.save({
            page: page.id,
            base: this.store.heads(page.id).slice(0, 1),
            path: target,
            text: page.text,
          });
          moved.push(page.id);
        }
      }
    });
    this.schedulePublish();
    return moved;
  }
  async close() {
    clearTimeout(this.retryTimer);
    await this.publish();
    this.store.close();
  }
}
// What two imports in a row changed, with the later one's view of what is
// still wrong.
const combine = (first: ImportReport, last: ImportReport): ImportReport => ({
  ...last,
  applied: first.applied + last.applied,
  pages: new Set([...first.pages, ...last.pages]),
  conflicts: new Set([...first.conflicts, ...last.conflicts]),
  overlapped: new Set([...first.overlapped, ...last.overlapped]),
  missingAttachments: [
    ...new Set([...first.missingAttachments, ...last.missingAttachments]),
  ],
});
const overlapWarning = (page: string) =>
  `${page} was edited in two places at once and some of the same lines changed both ways. Quire kept both versions of those lines next to each other; check the page and remove the one you don't want.`;
const conflictWarning = (page: string) =>
  `${page} was changed on two computers from the same version. Both versions are kept; the newer one is shown. Open page history to compare and combine them.`;
