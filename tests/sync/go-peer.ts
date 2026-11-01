// The Go terminal app's sync engine (`rt` tui/cmd/quire `peer`) as one harness computer, so
// native schedules run against Go as rt's sync-go-harness test runs TypeScript against it.
// Requests and replies are JSON lines on two named pipes; while answering, the peer asks for
// folder operations, which run against the simulated network.
import { execFileSync, spawn, type ChildProcess } from "node:child_process";
import { closeSync, mkdtempSync, openSync, readSync, rmSync, writeSync } from "node:fs";
import { join } from "node:path";
import { StringDecoder } from "node:string_decoder";
import type { Revision } from "../reference/sync/format";
import type { ImportReport } from "../reference/sync/replica";
import type { EngineFactory, SyncFolder, TestReplica, TestStore } from "./harness";

type FolderRequest = { folder?: string; path?: string; data?: string; result?: unknown; error?: string };

export class GoPeer {
  private child: ChildProcess;
  private toGo: number;
  private fromGo: number;
  private pending = "";
  private decoder = new StringDecoder("utf8");
  constructor(binary: string, readonly directory: string, notebook: string, device: string, private folder: SyncFolder, storage: "memory" | "sqlite") {
    const requests = join(directory, "requests"), replies = join(directory, "replies");
    execFileSync("mkfifo", [requests, replies]);
    const store = storage === "memory" ? "memory" : join(directory, "notebook.sqlite");
    this.child = spawn(binary, ["peer", requests, replies, notebook, device, store], { stdio: ["ignore", "inherit", "inherit"] });
    // Both sides open the request pipe first, so neither waits forever.
    this.toGo = openSync(requests, "w");
    this.fromGo = openSync(replies, "r");
  }
  private line() {
    while (!this.pending.includes("\n")) {
      const buffer = Buffer.alloc(1 << 16);
      const length = readSync(this.fromGo, buffer, 0, buffer.length, null);
      if (!length) throw new Error("The Go peer stopped. Read its output above, then rebuild it from the rt checkout.");
      this.pending += this.decoder.write(buffer.subarray(0, length));
    }
    const end = this.pending.indexOf("\n"), line = this.pending.slice(0, end);
    this.pending = this.pending.slice(end + 1);
    return line;
  }
  private write(value: unknown) {
    const bytes = Buffer.from(JSON.stringify(value) + "\n");
    for (let offset = 0; offset < bytes.length;) offset += writeSync(this.toGo, bytes, offset, bytes.length - offset);
  }
  request<T>(value: Record<string, unknown>): T {
    this.write(value);
    for (;;) {
      const message = JSON.parse(this.line()) as FolderRequest;
      if (!message.folder) {
        if (message.error !== undefined) throw new Error(message.error);
        return message.result as T;
      }
      try {
        const path = message.path!;
        if (message.folder === "list") this.write({ names: this.folder.list(path) });
        else if (message.folder === "read") {
          const data = this.folder.read(path);
          this.write({ data: data ? Buffer.from(data).toString("base64") : null });
        } else if (message.folder === "exists") this.write({ exists: this.folder.exists(path) });
        else {
          this.folder.write(path, new Uint8Array(Buffer.from(message.data!, "base64")));
          this.write({});
        }
      } catch (error) {
        this.write({ error: String((error as Error).message ?? error) });
      }
    }
  }
  close() {
    try { this.request({ op: "quit" }); } catch { /* Already stopped. */ }
    closeSync(this.toGo);
    closeSync(this.fromGo);
    this.child.kill();
  }
}

const peers = new Set<GoPeer>();
export function closeGoPeers() {
  for (const peer of peers) {
    peer.close();
    rmSync(peer.directory, { recursive: true, force: true });
  }
  peers.clear();
}

// A harness engine backed by a fresh Go peer under `root`.
export const goEngine = (binary: string, root: string, device: string, storage: "memory" | "sqlite", notebook = "nb-test"): EngineFactory =>
  (folder, clock) => {
    const peer = new GoPeer(binary, mkdtempSync(join(root, "go-")), notebook, device, folder, storage);
    peers.add(peer);
    const store: TestStore = {
      get: (id) => peer.request<Revision | null>({ op: "get", id }) ?? undefined,
      heads: (page) => peer.request<string[]>({ op: "heads", page }),
      pages: () => peer.request<string[]>({ op: "pages" }),
      outbox: () => peer.request<string[]>({ op: "outbox" }),
    };
    const head = (page: string) => {
      const id = store.heads(page)[0];
      return id ? store.get(id) : undefined;
    };
    const replica: TestReplica = {
      head,
      heads: (page) => store.heads(page).map((id) => store.get(id)!),
      conflicts: () => peer.request<string[]>({ op: "conflicts" }),
      save: (edit) => {
        const result = peer.request<{ revision: Revision; replaced: string | null; saved: boolean; heads: string[]; conflict: boolean; merged: Revision | null; overlaps: number }>({ op: "save", now: clock(), ...edit });
        return { ...result, replaced: result.replaced ?? undefined, merged: result.merged ?? undefined };
      },
      publish: async () => peer.request<{ revisions: number }>({ op: "publish", now: clock() }),
      import: async (): Promise<ImportReport> => {
        const report = peer.request<{ applied: number; pages: string[]; overlapped: string[] }>({ op: "import" });
        return { applied: report.applied, pages: new Set(report.pages), conflicts: new Set(), problems: [], waiting: [], missingAttachments: [], overlapped: new Set(report.overlapped) };
      },
    };
    return { store, replica, rebuild: () => peer.request({ op: "rebuild" }) };
  };
