import type {
  DiskDocument,
  FileEdit,
  Recovery,
  RemoveRequest,
  RevisionInfo,
  SaveResult,
  VaultSnapshot,
} from "../core/types";
import type { TreeEntry } from "../core/workspace";

// What the desktop app needs from an open notebook. Folder notebooks
// (VaultFiles) keep Markdown files; Quire notebooks (EngineFiles) keep a
// local database and a synced change log.
export interface NotebookFiles {
  readonly kind: "folder" | "quire";
  readonly root: string;
  // Where this computer keeps the notebook's local databases.
  readonly dataDirectory: string;
  readonly searchFile: string;
  snapshot(): Promise<VaultSnapshot>;
  save(
    relative: string,
    text: string,
    expectedRevision: string | null,
  ): Promise<SaveResult>;
  commitEdits(edits: FileEdit[]): Promise<DiskDocument[]>;
  recover(draft: Recovery): Promise<void>;
  clearRecovery(relative: string, onlyText?: string): Promise<void>;
  revisions(relative: string): RevisionInfo[] | Promise<RevisionInfo[]>;
  revisionText(relative: string, id: number): string | Promise<string>;
  attach(source: string): Promise<string>;
  createFolder(relative: string): Promise<unknown> | void;
  // Deletes the pages, refusing all of them when one changed since the
  // writer confirmed; returns the new tree. `trash` moves a file or folder
  // to the system Trash.
  remove(
    request: RemoveRequest,
    trash: (absolute: string) => Promise<void>,
  ): Promise<TreeEntry[]>;
  writeNotebookSettings(text: string): Promise<void>;
  revealPath(relative: string): Promise<string>;
  exclusive<T>(task: () => Promise<T>): Promise<T>;
  close(): void | Promise<void>;
}
