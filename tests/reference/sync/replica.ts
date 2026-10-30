import {
  attachmentFileName,
  decodeBatch,
  encodeBatch,
  format,
  parseName,
  statusProblem,
  type Cursor,
  type DeviceStatus,
  type Revision,
} from "./format";
import {
  addRevision,
  MemoryRevisions,
  chainStep,
  createRevision,
  mergeHeads,
  sameDevice,
  type RevisionStore,
} from "./graph";

// The synced folder as one computer sees it. Paths use "/". `read`
// returns undefined while a sync tool has listed a file but not delivered
// its content. `write` must be atomic and durable.
export interface Folder {
  list(directory: string): Promise<string[]>;
  read(path: string): Promise<Uint8Array | undefined>;
  write(path: string, data: Uint8Array): Promise<void>;
  exists(path: string): Promise<boolean>;
}
// This computer's durable state. `transaction` must apply all or nothing.
export interface ReplicaStore extends RevisionStore {
  pages(): string[];
  cursor(device: string): Cursor | undefined;
  setCursor(device: string, cursor: Cursor): void;
  outbox(): string[];
  addOutbox(id: string): void;
  removeOutbox(ids: string[]): void;
  inOutbox(id: string): boolean;
  // Forgets an unpublished revision that a newer save replaces.
  discard(id: string): void;
  transaction<T>(run: () => T): T;
}

export type Problem = { device: string; file?: string; reason: string };
export type ImportReport = {
  applied: number;
  pages: Set<string>;
  conflicts: Set<string>;
  problems: Problem[];
  waiting: { device: string; reason: string }[];
  missingAttachments: string[];
  // Pages merged where both sides changed the same lines, so both versions
  // of those lines were kept.
  overlapped: Set<string>;
};

const heartbeatEvery = 60_000;
// Large imports are split so each file stays a few megabytes.
const batchBytes = 4 * 2 ** 20,
  batchRevisions = 2_000;
const logDirectory = (device: string) => `devices/${device}/log`;
const attachmentReference = /attachments\/([0-9a-f]{64}(?:\.[a-z0-9]{1,10})?)/g;
export const referencedAttachments = (texts: string[]) =>
  [
    ...new Set(
      texts.flatMap((text) =>
        [...text.matchAll(attachmentReference)].map((match) => match[1]),
      ),
    ),
  ].sort();

export class Replica {
  readonly notebook: string;
  readonly device: string;
  private folder: Folder;
  private store: ReplicaStore;
  private now: () => number;
  private label: string;
  private app: string;
  // True until a publish confirms that no batch was written without being
  // recorded, for example by a crash in the previous run.
  private uncertain = true;
  // Saves that publish() is writing. A save made meanwhile must not
  // replace them: the file being written already holds them.
  private publishing = new Set<string>();
  // Revisions already looked for in this computer's own change files.
  private searched = new Set<string>();
  // The status last written, so an unchanged one is only refreshed as a
  // heartbeat.
  private written?: { outbox: number; at: number };
  constructor(options: {
    notebook: string;
    device: string;
    folder: Folder;
    store: ReplicaStore;
    now?: () => number;
    label?: string;
    app?: string;
  }) {
    this.notebook = options.notebook;
    this.device = options.device;
    this.folder = options.folder;
    this.store = options.store;
    this.now = options.now ?? Date.now;
    this.label = options.label ?? options.device;
    this.app = options.app ?? "";
  }

  head(page: string) {
    const id = this.store.heads(page)[0];
    return id ? this.store.get(id) : undefined;
  }
  heads(page: string) {
    return this.store
      .heads(page)
      .map((id) => this.store.get(id)!)
      .filter(Boolean);
  }
  conflicts() {
    return this.store
      .pages()
      .filter((page) => this.store.heads(page).length > 1);
  }
  // Combines a page's heads into one revision when it has several.
  private mergePage(page: string) {
    const merged = mergeHeads(this.store, page, {
      created: this.now(),
      device: this.device,
    });
    if (!merged) return undefined;
    const added = addRevision(this.store, merged.revision);
    if (added.kind !== "added")
      throw new Error(
        `Two versions of a page could not be combined: ${added.kind === "invalid" ? added.reason : "a version is missing"}. Reopen the notebook to try again.`,
      );
    this.store.addOutbox(merged.revision.id);
    return merged;
  }
  // Merges each page that has competing versions and returns the pages
  // whose overlapping lines were kept twice.
  private mergePages(pages: Iterable<string>, overlapped = new Set<string>()) {
    this.store.transaction(() => {
      for (const page of pages)
        if (this.mergePage(page)?.overlaps) overlapped.add(page);
    });
    return overlapped;
  }
  // Merges the pages that still have competing versions, such as ones kept
  // by an older Quire.
  mergeConflicts() {
    return this.mergePages(this.conflicts());
  }

  // Saves an edit made from `base` (the revisions the editor started
  // from). Saving unchanged text from the current head writes nothing, so
  // an idle window can never re-save an old version.
  save(edit: {
    page: string;
    base: string[];
    path: string;
    text: string;
    deleted?: boolean;
  }) {
    const deleted = edit.deleted ?? false;
    return this.store.transaction(() => {
      const heads = this.store.heads(edit.page);
      if (edit.base.length === 1) {
        const base = this.store.get(edit.base[0]);
        if (
          base &&
          base.text === edit.text &&
          base.path === edit.path &&
          base.deleted === deleted
        )
          return {
            revision: base,
            heads,
            conflict: heads.length > 1,
            saved: false,
          };
      }
      // Nobody else has seen an unpublished revision, so a newer save from
      // it replaces it: one revision per publish, not one per autosave.
      // Skipped after a publish that may have written a file without
      // recording it, when "unpublished" is not certain. A page's first
      // version and a merge are never replaced: another computer can make
      // the identical revision, and edits that follow must descend from it.
      const previous =
        edit.base.length === 1 ? this.store.get(edit.base[0]) : undefined;
      const replaces =
        !this.uncertain &&
        !!previous &&
        previous.device === this.device &&
        previous.parents.length === 1 &&
        heads.length === 1 &&
        heads[0] === previous.id &&
        this.store.inOutbox(previous.id) &&
        !this.publishing.has(previous.id)
          ? previous
          : undefined;
      if (replaces) this.store.discard(replaces.id);
      const revision = createRevision(this.store, {
        page: edit.page,
        parents: replaces ? replaces.parents : edit.base,
        path: edit.path,
        deleted,
        text: edit.text,
        created: this.now(),
        device: this.device,
      });
      const added = addRevision(this.store, revision);
      if (added.kind === "duplicate")
        return { revision, heads, conflict: heads.length > 1, saved: false };
      if (added.kind !== "added")
        throw new Error(
          `The page could not be saved: ${added.kind === "invalid" ? added.reason : "its previous version is missing"}. Reopen the page and save again.`,
        );
      this.store.addOutbox(revision.id);
      // Saved from a version another computer had moved past: merge.
      const merged = this.mergePage(edit.page);
      return {
        revision,
        replaced: replaces?.id,
        heads: merged ? [merged.revision.id] : added.heads,
        conflict: merged ? false : added.conflict,
        saved: true,
        merged: merged?.revision,
        overlaps: merged?.overlaps ?? 0,
      };
    });
  }

  // Attachments are written before the revision that links them, under a
  // name derived from their content, so they never need updating.
  async attach(data: Uint8Array, original: string) {
    const name = attachmentFileName(data, original);
    if (!(await this.folder.exists(`attachments/${name}`)))
      await this.folder.write(`attachments/${name}`, data);
    return `attachments/${name}`;
  }

  // Publishes every saved revision not yet in the folder as one batch.
  async publish(): Promise<{ name?: string; revisions: number }> {
    for (let attempt = 0; attempt < 3; attempt++) {
      const ids = this.batchPrefix(this.store.outbox());
      if (!ids.length) {
        this.uncertain = false;
        return { revisions: 0 };
      }
      ids.forEach((id) => this.publishing.add(id));
      try {
        const published = await this.publishBatch(ids);
        if (published) return published;
      } finally {
        ids.forEach((id) => this.publishing.delete(id));
      }
    }
    throw new Error(
      "This computer's change files could not be read back. Wait for the sync tool to finish and try again.",
    );
  }

  // Writes one batch, or returns undefined after adopting a batch that an
  // earlier run wrote without recording it.
  private async publishBatch(ids: string[]) {
    const mine = this.store.cursor(this.device);
    const seq = (mine?.seq ?? 0) + 1;
    // A batch written before a crash, but not yet recorded, is adopted
    // rather than written again with a different hash.
    const written = (await this.folder.list(logDirectory(this.device))).some(
      (name) => parseName(name, "qlog")?.seq === seq,
    );
    if (written) {
      await this.importDevice(this.device, this.emptyReport());
      return undefined;
    }
    const revisions = ids.map((id) => this.store.get(id)!);
    const encoded = encodeBatch({
      notebook: this.notebook,
      device: this.device,
      seq,
      previous: mine?.hash ?? null,
      created: this.now(),
      revisions,
      attachments: referencedAttachments(revisions.map((r) => r.text)),
    });
    try {
      await this.folder.write(
        `${logDirectory(this.device)}/${encoded.name}`,
        encoded.data,
      );
      this.store.transaction(() => {
        this.store.setCursor(this.device, { seq, hash: encoded.hash });
        this.store.removeOutbox(ids);
      });
    } catch (error) {
      this.uncertain = true;
      throw error;
    }
    this.uncertain = false;
    return { name: encoded.name, revisions: ids.length };
  }

  // The oldest saves that fit one batch. Saves are in order, so a prefix
  // never needs a revision from a later batch.
  private batchPrefix(ids: string[]) {
    let bytes = 0;
    const taken: string[] = [];
    for (const id of ids) {
      bytes += this.store.get(id)!.text.length;
      if (
        taken.length &&
        (bytes > batchBytes || taken.length >= batchRevisions)
      )
        break;
      taken.push(id);
    }
    return taken;
  }
  // Writes status.json when the number of changes waiting to publish
  // differs from the last one written, or as a heartbeat once a minute.
  // The sync tool uploads every rewrite and other apps wake for it.
  async writeStatus() {
    const outbox = this.store.outbox().length,
      at = this.now();
    if (
      this.written?.outbox === outbox &&
      at - this.written.at < heartbeatEvery
    )
      return;
    const status: DeviceStatus = {
      format,
      app: this.app,
      device: this.device,
      label: this.label,
      lastSeq: this.store.cursor(this.device)?.seq ?? 0,
      outbox,
      heartbeat: at,
    };
    await this.folder.write(
      `devices/${this.device}/status.json`,
      new TextEncoder().encode(JSON.stringify(status)),
    );
    this.written = { outbox, at };
  }
  // Whether another Quire app is running on this computer: a status with
  // this computer's label and a heartbeat from the last two minutes.
  nearby(statuses: DeviceStatus[]) {
    const now = this.now();
    return statuses.some(
      (status) =>
        status.device !== this.device &&
        status.label === this.label &&
        now - status.heartbeat < 2 * heartbeatEvery,
    );
  }
  async statuses() {
    const result: DeviceStatus[] = [];
    for (const device of await this.folder.list("devices")) {
      const data = await this.folder.read(`devices/${device}/status.json`);
      if (!data) continue;
      try {
        const value = JSON.parse(new TextDecoder().decode(data));
        if (!statusProblem(value) && value.device === device)
          result.push(value);
      } catch {
        // A status file mid-delivery is read again next time.
      }
    }
    return result;
  }

  // Before 0.9.50 a save could replace a revision while publish() was
  // writing it, so this computer forgot a revision its change files hold.
  // When another computer builds on one, it is read back from those files,
  // looking for each revision once per run.
  private async restoreOwn(missing: string[]) {
    const wanted = new Set(missing.filter((id) => !this.searched.has(id)));
    if (!wanted.size) return false;
    wanted.forEach((id) => this.searched.add(id));
    const directory = logDirectory(this.device);
    const found: Revision[] = [];
    for (const name of (await this.folder.list(directory)).sort()) {
      if (!parseName(name, "qlog")) continue;
      const data = await this.folder.read(`${directory}/${name}`);
      const decoded = data
        ? decodeBatch(name, data, {
            notebook: this.notebook,
            device: this.device,
          })
        : undefined;
      if (decoded?.ok)
        found.push(...decoded.value.revisions.filter((r) => wanted.has(r.id)));
    }
    return this.store.transaction(() =>
      found
        .map((revision) => addRevision(this.store, revision).kind)
        .includes("added"),
    );
  }

  private emptyReport(): ImportReport {
    return {
      applied: 0,
      pages: new Set(),
      conflicts: new Set(),
      problems: [],
      waiting: [],
      missingAttachments: [],
      overlapped: new Set(),
    };
  }
  // Applies every complete batch that has arrived, computer by computer,
  // until no more can be applied. A batch that needs another computer's
  // revision waits for it.
  async import(): Promise<ImportReport> {
    const report = this.emptyReport();
    const devices = (await this.folder.list("devices")).sort();
    for (let progress = true; progress;) {
      progress = false;
      report.waiting = [];
      report.problems = [];
      for (const device of devices)
        if (await this.importDevice(device, report)) progress = true;
    }
    this.mergePages(report.pages, report.overlapped);
    for (const page of report.pages)
      if (this.store.heads(page).length > 1) report.conflicts.add(page);
    return report;
  }

  private async importDevice(device: string, report: ImportReport) {
    let progressed = false;
    const names = (await this.folder.list(logDirectory(device)))
      .map((name) => ({ name, parsed: parseName(name, "qlog") }))
      .filter((entry) => entry.parsed);
    const bySeq = new Map<number, typeof names>();
    for (const entry of names) {
      const group = bySeq.get(entry.parsed!.seq) ?? [];
      group.push(entry);
      bySeq.set(entry.parsed!.seq, group);
    }
    const cursor = this.store.cursor(device);
    if (
      cursor &&
      bySeq.get(cursor.seq)?.some((entry) => entry.parsed!.hash !== cursor.hash)
    ) {
      report.problems.push({
        device,
        reason: sameDevice,
      });
      return false;
    }
    for (;;) {
      const at = this.store.cursor(device);
      const seq = (at?.seq ?? 0) + 1;
      const candidates = bySeq.get(seq) ?? [];
      if (!candidates.length) {
        if ([...bySeq.keys()].some((n) => n > seq))
          report.waiting.push({
            device,
            reason: `change ${seq} has not arrived`,
          });
        return progressed;
      }
      if (candidates.length > 1) {
        report.problems.push({
          device,
          reason: sameDevice,
        });
        return progressed;
      }
      const { name } = candidates[0];
      const data = await this.folder.read(`${logDirectory(device)}/${name}`);
      if (!data) {
        report.waiting.push({ device, reason: `${name} is still downloading` });
        return progressed;
      }
      const decoded = decodeBatch(name, data, {
        notebook: this.notebook,
        device,
      });
      if (!decoded.ok) {
        // A partly delivered file is read again on the next pass.
        report.problems.push({ device, file: name, reason: decoded.reason });
        return progressed;
      }
      const batch = decoded.value;
      const step = chainStep(at, batch, decoded.hash);
      if (step.kind === "fork") {
        report.problems.push({ device, file: name, reason: step.reason });
        return progressed;
      }
      if (step.kind !== "apply") return progressed;
      const inBatch = new Set<string>();
      const missing = batch.revisions.flatMap((revision) => {
        const absent = revision.parents.filter(
          (id) => !inBatch.has(id) && !this.store.get(id),
        );
        inBatch.add(revision.id);
        return absent;
      });
      if (missing.length) {
        if (await this.restoreOwn(missing)) continue;
        report.waiting.push({
          device,
          reason: `${name} follows changes that have not arrived`,
        });
        return progressed;
      }
      this.store.transaction(() => {
        for (const revision of batch.revisions) {
          const added = addRevision(this.store, revision);
          if (added.kind === "added") {
            report.applied++;
            report.pages.add(revision.page);
          }
        }
        // A revision waiting here that another computer has published (two
        // computers can make the same merge) is no longer this computer's
        // to publish or replace: others may build on it.
        this.store.removeOutbox(batch.revisions.map((r) => r.id));
        this.store.setCursor(device, { seq, hash: decoded.hash });
      });
      for (const attachment of batch.attachments)
        if (!(await this.folder.exists(`attachments/${attachment}`)))
          report.missingAttachments.push(attachment);
      progressed = true;
    }
  }
}

export class MemoryReplicaStore
  extends MemoryRevisions
  implements ReplicaStore
{
  private cursors = new Map<string, Cursor>();
  private pending: string[] = [];
  all() {
    return [...this.revisions.values()];
  }
  cursor(device: string) {
    return this.cursors.get(device);
  }
  setCursor(device: string, cursor: Cursor) {
    this.cursors.set(device, cursor);
  }
  outbox() {
    return [...this.pending];
  }
  addOutbox(id: string) {
    if (!this.pending.includes(id)) this.pending.push(id);
  }
  removeOutbox(ids: string[]) {
    const gone = new Set(ids);
    this.pending = this.pending.filter((id) => !gone.has(id));
  }
  inOutbox(id: string) {
    return this.pending.includes(id);
  }
  override discard(id: string) {
    super.discard(id);
    this.removeOutbox([id]);
  }
  transaction<T>(run: () => T) {
    return run();
  }
}
