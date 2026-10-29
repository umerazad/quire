import {
  revisionId,
  revisionProblem,
  type Cursor,
  type Revision,
} from "./format";

// Page versions form a graph: each revision names the revision(s) it was
// edited from. A page's heads are its revisions with no known successor.
// One head is the normal case; more than one means two computers edited
// from the same version, and every head is kept until someone merges.
export interface RevisionStore {
  get(id: string): Revision | undefined;
  heads(page: string): string[];
  // Stores the revision and the page's new heads (already in sortHeads
  // order) in one step.
  put(revision: Revision, heads: string[]): void;
}
export class MemoryRevisions implements RevisionStore {
  protected revisions = new Map<string, Revision>();
  private pageHeads = new Map<string, string[]>();
  get(id: string) {
    return this.revisions.get(id);
  }
  heads(page: string) {
    return this.pageHeads.get(page) ?? [];
  }
  put(revision: Revision, heads: string[]) {
    this.revisions.set(revision.id, revision);
    this.pageHeads.set(revision.page, heads);
  }
  pages() {
    return [...this.pageHeads.keys()].sort();
  }
  discard(id: string) {
    const revision = this.revisions.get(id);
    if (!revision) return;
    this.revisions.delete(id);
    this.pageHeads.set(
      revision.page,
      this.heads(revision.page).filter((head) => head !== id),
    );
  }
}

// Newest generation first, then by id, so every computer shows the same
// head first when a page has several.
// `adding` is a revision not stored yet.
export const sortHeads = (
  store: RevisionStore,
  ids: string[],
  adding?: Revision,
) => {
  const generation = (id: string) =>
    (id === adding?.id ? adding : store.get(id))?.generation ?? 0;
  return [...new Set(ids)].sort((a, b) => {
    const ga = generation(a),
      gb = generation(b);
    return gb - ga || (a < b ? 1 : a > b ? -1 : 0);
  });
};

export type AddResult =
  | { kind: "added"; heads: string[]; conflict: boolean }
  | { kind: "duplicate" }
  | { kind: "missing"; ids: string[] }
  | { kind: "invalid"; reason: string };
export function addRevision(
  store: RevisionStore,
  revision: Revision,
): AddResult {
  const problem = revisionProblem(revision);
  if (problem) return { kind: "invalid", reason: problem };
  if (store.get(revision.id)) return { kind: "duplicate" };
  const parents = revision.parents.map((id) => store.get(id));
  const missing = revision.parents.filter((_, i) => !parents[i]);
  if (missing.length) return { kind: "missing", ids: missing };
  if (parents.some((parent) => parent!.page !== revision.page))
    return { kind: "invalid", reason: "a revision's parent is another page" };
  if (revision.generation !== generationAfter(parents as Revision[]))
    return { kind: "invalid", reason: "a revision's generation is wrong" };
  // Parents stop being heads. A parent that was already superseded leaves
  // the newer head in place, which is exactly a conflict.
  const heads = sortHeads(
    store,
    [
      ...store
        .heads(revision.page)
        .filter((id) => !revision.parents.includes(id)),
      revision.id,
    ],
    revision,
  );
  store.put(revision, heads);
  return { kind: "added", heads, conflict: heads.length > 1 };
}
const generationAfter = (parents: Revision[]) =>
  parents.length ? Math.max(...parents.map((p) => p.generation)) + 1 : 0;

// Builds a new revision edited from `parents`. A save passes the revision
// its editor started from; resolving a conflict passes every head.
export function createRevision(
  store: RevisionStore,
  fields: {
    page: string;
    parents: string[];
    path: string;
    deleted?: boolean;
    text: string;
    created: number;
    device: string;
  },
): Revision {
  const parents = [...new Set(fields.parents)].sort();
  const known = parents.map((id) => {
    const parent = store.get(id);
    if (!parent)
      throw new Error(
        "This page's previous version is not available. Reopen the page and save again.",
      );
    return parent;
  });
  const base = {
    page: fields.page,
    parents,
    generation: generationAfter(known),
    path: fields.path,
    deleted: fields.deleted ?? false,
    text: fields.text,
  };
  return {
    ...base,
    id: revisionId(base),
    created: fields.created,
    device: fields.device,
  };
}

// True when `ancestor` is `id` or one of its predecessors. Generations
// only grow along edges, so the search stops below the target.
export function isAncestor(store: RevisionStore, ancestor: string, id: string) {
  const target = store.get(ancestor);
  if (!target) return false;
  const pending = [id],
    seen = new Set<string>();
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

export const sameDevice =
  "Two computers are writing as the same device. This happens when Quire's app data is copied between computers. Remove the copied device-id file on one of them and reopen.";
// Each computer's batches form a chain: batch n names the hash of n - 1.
export type ChainStep =
  | { kind: "apply" }
  | { kind: "applied" }
  | { kind: "gap"; expected: number }
  | { kind: "fork"; reason: string };
export function chainStep(
  cursor: Cursor | undefined,
  batch: { seq: number; previous: string | null },
  hash: string,
  known?: (seq: number) => string | undefined,
): ChainStep {
  const at = cursor?.seq ?? 0;
  if (batch.seq <= at) {
    const seen = batch.seq === at ? cursor!.hash : known?.(batch.seq);
    return seen === undefined || seen === hash
      ? { kind: "applied" }
      : { kind: "fork", reason: sameDevice };
  }
  if (batch.seq > at + 1) return { kind: "gap", expected: at + 1 };
  if (batch.previous !== (cursor?.hash ?? null))
    return {
      kind: "fork",
      reason:
        "A computer's change files do not follow on from each other. Keep its folder unchanged and reopen the notebook after the sync tool finishes.",
    };
  return { kind: "apply" };
}
