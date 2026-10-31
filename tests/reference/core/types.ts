import type { TreeEntry } from "./workspace";
import type { AiBackend, AiRequest, ExportFormat } from "./ai-commands";
import type { Appearance } from "./appearance";
// Commands the desktop menu sends to the renderer.
export type AppCommand = "undo" | "redo" | `appearance:${Appearance}`;
// A page sent for export, as standalone Markdown from exportMarkdown.
export type PageExport = {
  title: string;
  text: string;
  format: ExportFormat | "markdown";
};
export type DiskDocument = {
  path: string;
  text: string;
  revision: string;
  modified: number;
};
export type Recovery = {
  path: string;
  text: string;
  expectedRevision: string | null;
  savedAt: number;
  // Set when expectedRevision is the version the text was typed into, so
  // the draft can be merged with what changed since. Quire 0.9.52 and
  // older could record a newer version there.
  baseKnown?: boolean;
};
export type VaultSnapshot = {
  entries?: TreeEntry[];
  origin?: "roam";
  root: string;
  name: string;
  documents: DiskDocument[];
  warnings: string[];
  recovery: Recovery[];
  profileLabel?: string;
  // The notebook's own settings file, .quire/notebook.json, as written.
  notebookSettings?: string;
  // "quire" for a notebook kept in a local database and a synced log.
  storage?: "quire";
};
// One of a page's competing versions, saved on two computers from the
// same starting version.
export type ConflictVersion = {
  revision: string;
  text: string;
  savedAt: number;
  computer: string;
};
// Publishing state of a Quire notebook on this and other computers.
export type SyncStatus = {
  pending: number;
  computers: { label: string; seen: number; waiting: number }[];
};
export type ProfileId = "current" | "work" | "personal";
export type DataProfile = {
  id: ProfileId;
  label: string;
  root: string | null;
  backupDirectory: string | null;
  lastBackupAt: number | null;
  lastBackupPath: string | null;
  backupError: string | null;
  gitRemote: string | null;
  lastPushAt: number | null;
  gitError: string | null;
};
export type DataState = { active: ProfileId; profiles: DataProfile[] };
export type BackupInfo = {
  id: string;
  createdAt: number;
  path: string;
  notebook?: string;
};
export interface DataAPI {
  state(): Promise<DataState>;
  selectProfile(id: ProfileId): Promise<VaultSnapshot | null>;
  chooseNotebook(): Promise<VaultSnapshot | null>;
  chooseBackupDirectory(): Promise<DataState>;
  backup(): Promise<DataState>;
  backups(): Promise<BackupInfo[]>;
  restore(path: string): Promise<VaultSnapshot | null>;
  configureGit(remote: string): Promise<DataState>;
  syncGit(): Promise<VaultSnapshot>;
  createQuireNotebook?(): Promise<VaultSnapshot | null>;
  importMarkdown?(): Promise<VaultSnapshot | null>;
  syncStatus?(): Promise<SyncStatus | null>;
}
export type VaultEvent =
  | { type: "upsert"; document: DiskDocument }
  | { type: "delete"; path: string }
  | { type: "warning"; message: string };
export type SaveResult =
  // text is set when the save was merged with another computer's edits.
  | { ok: true; revision: string; warning?: string; text?: string }
  | { ok: false; message: string; disk: DiskDocument | null };
export type SearchHit = {
  path: string;
  title: string;
  snippet: string;
  blockId?: string;
};
export type RevisionInfo = {
  id: number;
  revision: string;
  savedAt: number;
};
export type ImportResult = { snapshot: VaultSnapshot; report: string[] };
export type ExportResult = { path: string; report: string[] };
// Pages to delete, each at the revision the writer confirmed, and the
// folder that held them when a whole folder goes.
export type RemoveRequest = {
  pages: { path: string; revision: string }[];
  folder?: string;
};
export type FileEdit = {
  remove?: boolean;
  retirement?: string;
  historyFrom?: string;
  path: string;
  text: string;
  expectedRevision: string | null;
};
export type AiEvent =
  | { kind: "token"; text: string }
  // A tool call starts with a detail; its result arrives with the same id.
  | {
      kind: "tool";
      id?: string;
      name: string;
      detail: string;
      result?: string;
      failed?: boolean;
    }
  | { kind: "session"; session: string }
  | { kind: "done"; text: string; session?: string }
  | { kind: "error"; message: string; retry?: boolean; canceled?: boolean };
export type AiSettings = {
  enabled: boolean;
  // Set once a backend test succeeds; the command stays off until then.
  tested: boolean;
  backend: AiBackend;
  // Empty means detect on the login shell's PATH.
  paths: Partial<Record<AiBackend, string>>;
  model: string;
  // Whether AI write tools are offered, staged for review, or applied at once.
  writes: AiWrites;
  // The registration command run per CLI by Use from a terminal.
  terminal: Partial<Record<AiBackend, string>>;
};
export type TerminalResult = {
  backend: AiBackend;
  ok: boolean;
  command?: string;
  message: string;
};
export type AiWrites = "off" | "review" | "apply";
export type ToolOutcome =
  | { ok: true; result: { summary: string; [key: string]: unknown } }
  | { ok: false; error: string };
// A tool call forwarded from the bridge to the window that owns the notebook.
export type NotebookCall = {
  name: string;
  input: unknown;
  run?: string;
  writes: AiWrites;
};
export type AiTestResult = {
  ok: boolean;
  version?: string;
  message: string;
  settings: AiSettings;
};
export interface AiAPI {
  settings(): Promise<AiSettings>;
  update(patch: Partial<AiSettings>): Promise<AiSettings>;
  test(backend?: AiBackend): Promise<AiTestResult>;
  registerTerminal(
    remove?: boolean,
  ): Promise<{ settings: AiSettings; results: TerminalResult[] }>;
  start(id: string, request: AiRequest): Promise<void>;
  // Asks where to save, then formats the page with the CLI; events report
  // progress and "done" carries the saved file's path.
  exportPage?(id: string, request: PageExport): Promise<void>;
  cancel(id: string): Promise<void>;
  onEvent(callback: (id: string, event: AiEvent) => void): () => void;
}
export interface DesktopAPI {
  ai?: AiAPI;
  notebook?: {
    onCall(callback: (call: NotebookCall) => Promise<ToolOutcome>): () => void;
  };
  windowControls?: {
    action(action: "close" | "minimize" | "maximize"): Promise<void>;
  };
  files?: {
    inventory(): Promise<TreeEntry[]>;
    createFolder(path: string): Promise<TreeEntry[]>;
    // Deletes pages, or a folder with its pages; resolves to the new tree.
    remove?(request: RemoveRequest): Promise<TreeEntry[]>;
    reveal(path: string): Promise<void>;
    // Saves the page as Markdown; resolves to the file, or null if canceled.
    exportMarkdown?(request: PageExport): Promise<string | null>;
    showFile?(file: string): Promise<void>;
    // Writes .quire/notebook.json, shared by every device on the notebook.
    saveNotebookSettings?(text: string): Promise<void>;
  };
  onNavigate?(callback: (direction: "back" | "forward") => void): () => void;
  data?: DataAPI;
  onCommand?(callback: (command: AppCommand) => void): () => void;
  // Light, dark or system, for native menus and the window background.
  appearance?: { set(value: Appearance): Promise<void> };
  startup(): Promise<VaultSnapshot>;
  openVault(): Promise<VaultSnapshot | null>;
  save(
    path: string,
    text: string,
    expectedRevision: string | null,
  ): Promise<SaveResult>;
  commitEdits(edits: FileEdit[]): Promise<DiskDocument[]>;
  // Quire notebooks only: competing versions of a page and their end.
  conflictVersions?(path: string): Promise<ConflictVersion[]>;
  resolveConflict?(path: string, text: string): Promise<DiskDocument>;
  recover(draft: Recovery): Promise<void>;
  clearRecovery(path: string, onlyText?: string): Promise<void>;
  revisions(path: string): Promise<RevisionInfo[]>;
  revisionText(path: string, id: number): Promise<string>;
  search(query: string): Promise<SearchHit[]>;
  revealVault(): Promise<void>;
  importRoam(): Promise<ImportResult | null>;
  exportRoam(): Promise<ExportResult | null>;
  attach(): Promise<string | null>;
  openLink(url: string): Promise<void>;
  onChange(callback: (event: VaultEvent) => void): () => void;
}

declare global {
  interface Window {
    desktop: DesktopAPI;
  }
}
