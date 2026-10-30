// A simulated synced folder and computers for testing the change log.
// Each computer has its own view of the folder, like a local OneDrive or
// Dropbox copy. A write lands in the writer's view at once and reaches the
// others later: delayed, duplicated, reordered, cut short, as a placeholder
// or as a sync-tool conflict copy.
import { pageIdForPath, type Revision } from "../reference/sync/format";
import {
  MemoryReplicaStore,
  Replica,
  type Folder,
  type ReplicaStore,
} from "../reference/sync/replica";

export const random = (seed: number) => {
  let state = seed >>> 0 || 1;
  return () => {
    state ^= state << 13;
    state ^= state >>> 17;
    state ^= state << 5;
    return (state >>> 0) / 2 ** 32;
  };
};

const placeholder = Symbol("placeholder");
type Entry = Uint8Array | typeof placeholder;
type Delivery = {
  to: string;
  path: string;
  data: Uint8Array;
  kind: "full" | "partial" | "placeholder" | "copy";
  version: number;
};
export class Network {
  views = new Map<string, Map<string, Entry>>();
  versions = new Map<string, Map<string, number>>();
  pending: Delivery[] = [];
  private version = 0;
  constructor(private rng: () => number) {}
  view(computer: string) {
    if (!this.views.has(computer)) {
      this.views.set(computer, new Map());
      this.versions.set(computer, new Map());
    }
    return this.views.get(computer)!;
  }
  write(from: string, path: string, data: Uint8Array) {
    const version = ++this.version;
    this.view(from).set(path, data);
    this.versions.get(from)!.set(path, version);
    for (const to of this.views.keys()) {
      if (to === from) continue;
      const r = this.rng();
      if (r < 0.15)
        this.pending.push({ to, path, data, kind: "placeholder", version });
      else if (r < 0.25)
        this.pending.push({ to, path, data, kind: "partial", version });
      if (this.rng() < 0.05)
        this.pending.push({ to, path, data, kind: "copy", version });
      this.pending.push({ to, path, data, kind: "full", version });
      if (this.rng() < 0.1)
        this.pending.push({ to, path, data, kind: "full", version });
    }
  }
  // Delivers one pending change, chosen at random, so order is not kept.
  deliverOne() {
    if (!this.pending.length) return false;
    const index = Math.floor(this.rng() * this.pending.length);
    const [delivery] = this.pending.splice(index, 1);
    this.apply(delivery);
    return true;
  }
  deliverAll() {
    // Partial and placeholder deliveries always have a full one queued.
    while (this.pending.length) this.deliverOne();
  }
  private apply({ to, path, data, kind, version }: Delivery) {
    const view = this.view(to),
      versions = this.versions.get(to)!;
    if (kind === "copy") {
      view.set(path.replace(/(\.[a-z]+\.zst|\.json)$/, " (1)$1"), data);
      return;
    }
    const current = versions.get(path) ?? 0,
      complete = view.get(path) === data;
    // A sync tool keeps the newest version of a changing file, and never
    // replaces a complete copy with a partial one of the same version.
    if (version < current || (version === current && complete)) return;
    view.set(
      path,
      kind === "full"
        ? data
        : kind === "placeholder"
          ? placeholder
          : data.subarray(0, Math.floor(data.length / 2)),
    );
    versions.set(path, version);
  }
  syncFolder(
    computer: string,
    hooks: { crashAfterWrite?: () => boolean } = {},
  ): SyncFolder {
    const view = this.view(computer);
    return {
      list: (directory) => {
        const prefix = directory + "/",
          names = new Set<string>();
        for (const path of view.keys())
          if (path.startsWith(prefix))
            names.add(path.slice(prefix.length).split("/")[0]);
        return [...names];
      },
      read: (path) => {
        const entry = view.get(path);
        return entry instanceof Uint8Array ? entry : undefined;
      },
      exists: (path) => view.has(path),
      write: (path, data) => {
        this.write(computer, path, data);
        if (hooks.crashAfterWrite?.())
          throw new Error("Simulated crash after the file was written");
      },
    };
  }
  folder(
    computer: string,
    hooks: { crashAfterWrite?: () => boolean } = {},
  ): Folder {
    const folder = this.syncFolder(computer, hooks);
    return {
      list: async (directory) => folder.list(directory),
      read: async (path) => folder.read(path),
      exists: async (path) => folder.exists(path),
      write: async (path, data) => folder.write(path, data),
    };
  }
}
export type SyncFolder = {
  [K in keyof Folder]: (
    ...args: Parameters<Folder[K]>
  ) => Awaited<ReturnType<Folder[K]>>;
};
// Another implementation of the sync engine, such as the Go peer.
export type TestReplica = Pick<Replica, "head" | "heads" | "conflicts" | "save" | "publish" | "import">;
export type Engine = { store: TestStore; replica: TestReplica; rebuild(): void };
export type EngineFactory = (folder: SyncFolder, clock: () => number) => Engine;

// Any store the replica can use; tests read revisions back through it.
export type TestStore = Pick<ReplicaStore, "get" | "heads" | "pages" | "outbox">;
// The frozen graph helper's traversal, with its interface narrowed to the reads it uses.
function isAncestor(store: Pick<TestStore, "get">, ancestor: string, id: string) {
  const target = store.get(ancestor);
  if (!target) return false;
  const pending = [id], seen = new Set<string>();
  while (pending.length) {
    const current = pending.pop()!;
    if (current === ancestor) return true;
    if (seen.has(current)) continue;
    seen.add(current);
    const revision = store.get(current);
    if (!revision || revision.generation <= target.generation) continue;
    pending.push(...revision.parents);
  }
  return false;
}
type Buffer = { base: string[]; text: string; path: string; dirty: boolean };
export class Computer {
  store: TestStore;
  replica: TestReplica;
  buffers = new Map<string, Buffer>();
  crashNext = false;
  constructor(
    public name: string,
    private network: Network,
    private notebook: string,
    private clock: () => number,
    // The folder view this computer uses; normally its own name.
    private viewName = name,
    private makeStore: () => ReplicaStore = () => new MemoryReplicaStore(),
    private makeEngine?: EngineFactory,
  ) {
    if (makeEngine) {
      this.engine = makeEngine(
        this.network.syncFolder(this.viewName, this.hooks),
        clock,
      );
      this.store = this.engine.store;
      this.replica = this.engine.replica;
      return;
    }
    const store = makeStore();
    this.store = store;
    this.replica = this.makeReplica(store);
  }
  private engine?: Engine;
  private hooks = {
    crashAfterWrite: () => {
      const crash = this.crashNext;
      this.crashNext = false;
      return crash;
    },
  };
  private makeReplica(store: ReplicaStore) {
    return new Replica({
      notebook: this.notebook,
      device: this.name,
      folder: this.network.folder(this.viewName, this.hooks),
      store,
      now: this.clock,
    });
  }
  // Deletes the local database and rebuilds it from the folder.
  async rebuild() {
    if (this.engine) this.engine.rebuild();
    else {
      const store = this.makeStore();
      this.store = store;
      this.replica = this.makeReplica(store);
    }
    this.buffers.clear();
    await this.replica.import();
  }
  open(page: string, path: string) {
    if (this.buffers.has(page)) return this.buffers.get(page)!;
    const head = this.replica.head(page);
    const buffer = {
      base: head ? [head.id] : [],
      text: head?.text ?? "- \n",
      path: head?.path ?? path,
      dirty: !head,
    };
    this.buffers.set(page, buffer);
    return buffer;
  }
  type(page: string, path: string, text: string) {
    const buffer = this.open(page, path);
    buffer.text = text;
    buffer.dirty = true;
  }
  // Saves the buffer even when nothing was typed, like an autosave timer.
  save(
    page: string,
    saved: Map<string, Revision>,
    options: { force?: boolean } = {},
  ) {
    const buffer = this.buffers.get(page);
    if (!buffer || (!buffer.dirty && !options.force)) return undefined;
    const result = this.replica.save({
      page,
      base: buffer.base,
      path: buffer.path,
      text: buffer.text,
    });
    if (result.saved) saved.set(result.revision.id, result.revision);
    if (result.replaced) saved.delete(result.replaced);
    buffer.base = [result.revision.id];
    // Saved from a version another computer moved past: the editor shows
    // the merge and continues from it.
    if (result.merged) {
      saved.set(result.merged.id, result.merged);
      buffer.base = [result.merged.id];
      buffer.text = result.merged.text;
      buffer.path = result.merged.path;
    }
    buffer.dirty = false;
    return result;
  }
  // Combines every head of a conflicted page into one version.
  resolve(page: string, saved: Map<string, Revision>) {
    const heads = this.replica.heads(page);
    if (heads.length < 2) return;
    const result = this.replica.save({
      page,
      base: heads.map((head) => head.id),
      path: heads[0].path,
      text: heads.map((head) => head.text).join(""),
    });
    saved.set(result.revision.id, result.revision);
    this.buffers.set(page, {
      base: [result.revision.id],
      text: result.revision.text,
      path: result.revision.path,
      dirty: false,
    });
  }
  async publish() {
    try {
      return await this.replica.publish();
    } catch (error) {
      if (!String(error).includes("Simulated crash")) throw error;
      return { revisions: 0 };
    }
  }
  // Imports and refreshes every open page that has no unsaved edits.
  async sync() {
    const report = await this.replica.import();
    for (const page of report.pages) {
      const buffer = this.buffers.get(page),
        head = this.replica.head(page);
      if (buffer && head && !buffer.dirty)
        this.buffers.set(page, {
          base: [head.id],
          text: head.text,
          path: head.path,
          dirty: false,
        });
    }
    return report;
  }
}

export const pagePaths = [
  "2026-10-29.md",
  "Plan.md",
  "People/Maram.md",
  "Inbox.md",
];
export async function runSchedule(
  seed: number,
  steps = 60,
  makeStore?: () => ReplicaStore,
  // Chooses another engine for some computers, by name.
  engineFor?: (name: string) => EngineFactory | undefined,
) {
  const rng = random(seed);
  const network = new Network(rng);
  let time = 1_000;
  const clock = () => ++time;
  const names = rng() < 0.5 ? ["office", "home"] : ["office", "home", "laptop"];
  for (const name of names) network.view(name);
  const computers = names.map(
    (name) =>
      new Computer(
        name,
        network,
        "nb-test",
        clock,
        name,
        makeStore,
        engineFor?.(name),
      ),
  );
  const saved = new Map<string, Revision>();
  const pick = <T>(items: T[]) => items[Math.floor(rng() * items.length)];
  let idleSaves = 0;
  // Hand-over runs edit on one computer at a time, as when you leave the
  // office and continue at home; the others edit anywhere at once.
  // Careful runs also wait for the folder to finish syncing at each
  // hand-over, which must never produce a conflict.
  const mode = pick(["chaos", "handover", "careful"] as const),
    handover = mode !== "chaos";
  let active = computers[0];
  for (let step = 0; step < steps; step++) {
    if (handover && rng() < 0.1) {
      const next = pick(computers);
      if (mode === "careful" && next !== active) {
        for (const page of active.buffers.keys()) active.save(page, saved);
        active.crashNext = false;
        await active.publish();
        network.deliverAll();
        await next.sync();
      }
      active = next;
    }
    const editor = handover ? active : pick(computers),
      computer = pick(computers),
      path = pick(pagePaths),
      page = pageIdForPath(path),
      roll = rng();
    if (roll < 0.3) editor.type(page, path, `- ${editor.name} ${step}\n`);
    else if (roll < 0.45) editor.save(page, saved);
    else if (roll < 0.5) {
      if (computer.buffers.has(page) && !computer.buffers.get(page)!.dirty) {
        const before = saved.size;
        computer.save(page, saved, { force: true });
        if (saved.size !== before)
          throw new Error("An idle autosave wrote a revision");
        idleSaves++;
      }
    } else if (roll < 0.6) {
      computer.crashNext = rng() < 0.2;
      await computer.publish();
    } else if (roll < 0.75) {
      // Import while files are still arriving.
      for (let i = 0; i < rng() * 4; i++) network.deliverOne();
      await computer.sync();
    } else if (roll < 0.9)
      for (let i = 0; i < 1 + rng() * 6; i++) network.deliverOne();
    else if (roll < 0.95) computer.resolve(page, saved);
    else if (
      computer.store.outbox().length === 0 &&
      [...computer.buffers.values()].every((buffer) => !buffer.dirty)
    )
      await computer.rebuild();
  }
  // Settle: everyone saves, publishes and imports until nothing moves.
  for (const computer of computers)
    for (const page of computer.buffers.keys()) computer.save(page, saved);
  // Merges made while settling are themselves published and imported.
  for (let round = 0; round < 12; round++) {
    for (const computer of computers) await computer.publish();
    network.deliverAll();
    for (const computer of computers) await computer.sync();
    if (
      round >= 3 &&
      computers.every((computer) => !computer.store.outbox().length)
    )
      break;
  }
  return { computers, saved, network, idleSaves, mode };
}

// Returns the first broken invariant, or undefined.
export function check({
  computers,
  saved,
}: Awaited<ReturnType<typeof runSchedule>>) {
  // Competing versions always merge.
  for (const computer of computers)
    if (computer.replica.conflicts().length)
      return `${computer.name} kept competing versions of ${computer.replica.conflicts().join(", ")}`;
  for (const computer of computers) {
    if (computer.store.outbox().length)
      return `${computer.name} still has unpublished revisions`;
    for (const id of saved.keys())
      if (!computer.store.get(id))
        return `${computer.name} lost revision ${id}`;
  }
  const [first, ...rest] = computers;
  for (const page of new Set(computers.flatMap((c) => c.store.pages()))) {
    const heads = first.store.heads(page).join();
    for (const other of rest)
      if (other.store.heads(page).join() !== heads)
        return `${first.name} and ${other.name} disagree about page ${page}`;
  }
  for (const revision of saved.values())
    if (
      !first.store
        .heads(revision.page)
        .some((head) => isAncestor(first.store, revision.id, head))
    )
      return `revision ${revision.id} was overwritten without a conflict`;
  return undefined;
}
