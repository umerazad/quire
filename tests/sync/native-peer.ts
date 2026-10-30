import { execFileSync, spawn, type ChildProcess } from "node:child_process";
import { closeSync, mkdirSync, openSync, readSync, writeFileSync, writeSync } from "node:fs";
import { join } from "node:path";
import { StringDecoder } from "node:string_decoder";
import type { Revision } from "../reference/sync/format";
import type { ImportReport } from "../reference/sync/replica";
import type { EngineFactory, SyncFolder, TestReplica, TestStore } from "./harness";

export class NativeProbe {
  private child: ChildProcess;
  private input: number;
  private output: number;
  private pending = "";
  private decoder = new StringDecoder("utf8");
  trace: unknown[] = [];
  requests = 0;
  constructor(binary: string, readonly directory: string) {
    const input = join(directory, "requests"), output = join(directory, "replies");
    execFileSync("mkfifo", [input, output]);
    this.child = spawn(binary, [input, output, directory], { stdio: ["ignore", "inherit", "inherit"] });
    this.input = openSync(input, "w");
    this.output = openSync(output, "r");
  }
  request<T>(request: Record<string, unknown>): T {
    this.trace.push({ request });
    this.requests++;
    const bytes = Buffer.from(JSON.stringify(request) + "\n");
    for (let offset = 0; offset < bytes.length;)
      offset += writeSync(this.input, bytes, offset, bytes.length - offset);
    while (!this.pending.includes("\n")) {
      const buffer = Buffer.alloc(1 << 16);
      const length = readSync(this.output, buffer, 0, buffer.length, null);
      if (!length) throw new Error("Native replica probe stopped. Retain the fixture and inspect its stderr.");
      this.pending += this.decoder.write(buffer.subarray(0, length));
    }
    const end = this.pending.indexOf("\n"), reply = JSON.parse(this.pending.slice(0, end));
    this.pending = this.pending.slice(end + 1);
    this.trace.push({ reply });
    if (reply.error !== undefined) throw new Error(reply.error);
    return reply.result as T;
  }
  close() {
    try { this.request({ op: "quit" }); }
    finally {
      closeSync(this.input);
      closeSync(this.output);
      this.child.kill();
    }
  }
}

function mirror(folder: SyncFolder, root: string, device?: string) {
  for (const writer of folder.list("devices")) {
    if (device !== undefined && writer !== device) continue;
    for (const name of folder.list(`devices/${writer}/log`)) {
      const relative = `devices/${writer}/log/${name}`;
      const directory = join(root, "devices", writer, "log");
      mkdirSync(directory, { recursive: true });
      // Listed provider placeholders stay invalid until their full immutable bytes arrive.
      writeFileSync(join(directory, name), folder.read(relative) ?? new Uint8Array());
    }
  }
  for (const name of folder.list("attachments")) {
    const directory = join(root, "attachments");
    mkdirSync(directory, { recursive: true });
    writeFileSync(join(directory, name), folder.read(`attachments/${name}`) ?? new Uint8Array());
  }
}

export const nativeEngine = (probe: NativeProbe, device: string, notebook = "nb-test", peer = device): EngineFactory =>
  (folder, clock) => {
    const { directory } = probe.request<{ directory: string }>({ op: "open", peer, notebook, device, now: clock() });
    const pages = new Set<string>();
    const request = <T>(op: string, fields: Record<string, unknown> = {}) => probe.request<T>({ op, peer, ...fields });
    const store: TestStore = {
      get: (id) => {
        const value = request<string | null>("get", { id });
        if (value === null) return undefined;
        const revision: Revision = JSON.parse(value);
        pages.add(revision.page);
        return revision;
      },
      heads: (page) => request<string[]>("heads", { page }),
      pages: () => request<string[]>("pages", { pages: [...pages].sort() }),
      outbox: () => request<string[]>("outbox"),
    };
    const head = (page: string) => {
      const id = store.heads(page)[0];
      return id ? store.get(id) : undefined;
    };
    const replica: TestReplica = {
      head,
      heads: (page) => store.heads(page).map((id) => store.get(id)!),
      conflicts: () => store.pages().filter((page) => store.heads(page).length > 1),
      save: (edit) => {
        const result = request<{ revision: string; heads: string[]; conflict: boolean; saved: boolean; replaced: string | null; merged: string | null; overlaps: number }>("save", {
          edit: { ...edit, deleted: edit.deleted ?? false, created: clock(), recovery: null },
        });
        const revision: Revision = JSON.parse(result.revision);
        pages.add(revision.page);
        const merged: Revision | undefined = result.merged === null ? undefined : JSON.parse(result.merged);
        return { ...result, revision, replaced: result.replaced ?? undefined, merged };
      },
      publish: async () => {
        mirror(folder, join(directory, "outgoing"), device);
        const selection = request<{ pending: { name: string; data: number[]; revisions: number; attachments: { name: string; data: number[] }[] } | null; pages: string[] }>("select", { now: clock() });
        for (const page of selection.pages) pages.add(page);
        const selected = selection.pending;
        if (selected === null) return { revisions: 0 };
        try {
          for (const attachment of selected.attachments)
            if (!folder.exists(`attachments/${attachment.name}`))
              folder.write(`attachments/${attachment.name}`, Uint8Array.from(attachment.data));
          folder.write(`devices/${device}/log/${selected.name}`, Uint8Array.from(selected.data));
        } catch (error) {
          request("uncertain");
          throw error;
        }
        mirror(folder, join(directory, "outgoing"), device);
        return request<{ revisions: number }>("complete", { now: clock() });
      },
      import: async (): Promise<ImportReport> => {
        mirror(folder, join(directory, "incoming"));
        const report = request<Omit<ImportReport, "pages" | "conflicts" | "overlapped"> & { pages: string[]; overlapped: string[] }>("import", { now: clock() });
        for (const page of report.pages) pages.add(page);
        return { ...report, pages: new Set(report.pages), conflicts: new Set(replica.conflicts()), overlapped: new Set(report.overlapped) };
      },
    };
    return { store, replica, rebuild: () => request("rebuild", { now: clock() }) };
  };
