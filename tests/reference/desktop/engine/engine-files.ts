import { promises as fs } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { notebookPath } from "../../core/file-paths";
import type { FileEdit, Recovery, RemoveRequest } from "../../core/types";
import type { NotebookFiles } from "../notebook-files";
import { NotebookEngine } from "./engine";

// Serves a Quire notebook to the desktop app through the same calls a
// folder notebook answers.
export class EngineFiles implements NotebookFiles {
  readonly kind = "quire";
  private queue: Promise<unknown> = Promise.resolve();
  constructor(readonly engine: NotebookEngine) {}
  get root() {
    return this.engine.root;
  }
  get dataDirectory() {
    return path.dirname(this.engine.databaseFile);
  }
  get searchFile() {
    return path.join(this.dataDirectory, "search.sqlite");
  }
  // Runs one task at a time against the notebook, in order.
  exclusive<T>(task: () => Promise<T>): Promise<T> {
    const next = this.queue.then(task, task);
    this.queue = next.catch(() => {});
    return next;
  }
  async snapshot() {
    return this.engine.snapshot();
  }
  save(relative: string, text: string, expectedRevision: string | null) {
    return this.exclusive(async () =>
      this.engine.save(relative, text, expectedRevision),
    );
  }
  commitEdits(edits: FileEdit[]) {
    return this.exclusive(async () => this.engine.commitEdits(edits));
  }
  async recover(draft: Recovery) {
    this.engine.recover(draft);
  }
  async clearRecovery(relative: string, onlyText?: string) {
    this.engine.clearRecovery(relative, onlyText);
  }
  revisions(relative: string) {
    return this.engine.revisions(relative);
  }
  revisionText(relative: string, id: number) {
    return this.engine.revisionText(relative, id);
  }
  async attach(source: string) {
    return this.engine.replica.attach(
      new Uint8Array(await fs.readFile(source)),
      path.basename(source),
    );
  }
  createFolder(relative: string) {
    this.engine.createFolder(relative);
  }
  // Records each deletion in the change log, so other computers remove
  // the page too; nothing goes to the Trash.
  remove(request: RemoveRequest) {
    return this.exclusive(async () => {
      this.engine.remove(request);
      return (await this.engine.snapshot()).entries ?? [];
    });
  }
  async writeNotebookSettings(text: string) {
    this.engine.writeNotebookSettings(text);
  }
  // Pages have no file of their own, so revealing one writes a copy to a
  // temporary folder. Edits to that copy do not return to the notebook.
  async revealPath(relative: string) {
    if (!relative) return this.root;
    const page = this.engine.read(notebookPath(relative));
    if (!page)
      throw new Error("This page is not in the notebook. Choose another page.");
    const folder = path.join(tmpdir(), "Quire pages");
    await fs.mkdir(folder, { recursive: true });
    const target = path.join(folder, path.basename(page.path));
    await fs.writeFile(target, page.text, "utf8");
    return target;
  }
  close() {
    return this.engine.close();
  }
}
