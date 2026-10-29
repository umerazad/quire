import { createHash } from "node:crypto";
import zlib from "node:zlib";

// The synced change log. Each computer appends batches of page revisions
// to its own folder; nothing is edited after it is written. Bump `format`
// only with a reader for every older value: notebooks keep old files.
export const format = 1;
export const maxBatchBytes = 256 * 2 ** 20;

export type Revision = {
  id: string;
  page: string;
  parents: string[];
  generation: number;
  path: string;
  deleted: boolean;
  text: string;
  created: number;
  device: string;
  [extra: string]: unknown;
};
export type Batch = {
  format: number;
  notebook: string;
  device: string;
  seq: number;
  previous: string | null;
  created: number;
  revisions: Revision[];
  attachments: string[];
  [extra: string]: unknown;
};
export type Cursor = { seq: number; hash: string };
export type Checkpoint = {
  format: number;
  notebook: string;
  device: string;
  seq: number;
  created: number;
  cursors: Record<string, Cursor>;
  heads: Revision[];
  [extra: string]: unknown;
};
export type DeviceStatus = {
  format: number;
  app: string;
  device: string;
  label: string;
  lastSeq: number;
  outbox: number;
  heartbeat: number;
};

type Json = null | boolean | number | string | Json[] | { [key: string]: Json };
// Sorted keys and no whitespace, so equal values always hash the same.
export function canonical(value: unknown): string {
  if (value === null || typeof value === "boolean" || typeof value === "string")
    return JSON.stringify(value);
  if (typeof value === "number") {
    if (!Number.isFinite(value))
      throw new Error("Change records cannot hold infinite numbers.");
    return JSON.stringify(value);
  }
  if (Array.isArray(value)) return `[${value.map(canonical).join(",")}]`;
  if (typeof value === "object")
    return `{${Object.keys(value)
      .filter((key) => (value as Record<string, unknown>)[key] !== undefined)
      .sort()
      .map(
        (key) =>
          `${JSON.stringify(key)}:${canonical((value as Record<string, unknown>)[key])}`,
      )
      .join(",")}}`;
  throw new Error(`Change records cannot hold ${typeof value} values.`);
}
export const sha256 = (value: string | Uint8Array) =>
  createHash("sha256").update(value).digest("hex");

// A revision's id covers everything that makes it the same edit, so two
// computers that produce identical text from the same parent agree.
export function revisionId(fields: {
  page: string;
  parents: string[];
  generation: number;
  path: string;
  deleted: boolean;
  text: string;
}) {
  return sha256(
    canonical({
      format,
      page: fields.page,
      parents: [...fields.parents].sort(),
      generation: fields.generation,
      path: fields.path,
      deleted: fields.deleted,
      text: fields.text,
    }),
  );
}

// Pages created by name get an id from their path, so a daily note made
// on two computers is one page. Others get a random id.
export const pageIdForPath = (path: string) =>
  "p-" + sha256(path.normalize("NFC").toLowerCase()).slice(0, 32);

const hex64 = /^[0-9a-f]{64}$/;
const isString = (value: unknown): value is string => typeof value === "string";
const isCount = (value: unknown): value is number =>
  Number.isSafeInteger(value) && (value as number) >= 0;
export function revisionProblem(value: unknown): string | undefined {
  if (!value || typeof value !== "object") return "a revision is not an object";
  const r = value as Record<string, unknown>;
  if (!isString(r.page) || !r.page) return "a revision has no page";
  if (
    !Array.isArray(r.parents) ||
    r.parents.length > 16 ||
    !r.parents.every((id) => isString(id) && hex64.test(id)) ||
    new Set(r.parents).size !== r.parents.length
  )
    return "a revision has invalid parents";
  if (!isCount(r.generation)) return "a revision has an invalid generation";
  if (!isString(r.path) || !isString(r.text) || typeof r.deleted !== "boolean")
    return "a revision has invalid content";
  if (!isCount(r.created) || !isString(r.device))
    return "a revision has an invalid origin";
  if (r.id !== revisionId(r as Revision))
    return "a revision's id does not match its content";
  return undefined;
}

const pack = (value: unknown) => {
  const text = canonical(value);
  return {
    hash: sha256(text),
    data: zlib.zstdCompressSync(Buffer.from(text, "utf8")),
  };
};
export type Damaged = { ok: false; reason: string };
function unpack(
  data: Uint8Array,
  hash: string,
): { ok: true; value: Record<string, Json> } | Damaged {
  let text: Buffer;
  try {
    text = zlib.zstdDecompressSync(data, { maxOutputLength: maxBatchBytes });
  } catch {
    return { ok: false, reason: "the file is incomplete or not compressed" };
  }
  if (sha256(text) !== hash)
    return { ok: false, reason: "the file's content does not match its name" };
  try {
    const value = JSON.parse(text.toString("utf8"));
    if (!value || typeof value !== "object" || Array.isArray(value))
      return { ok: false, reason: "the file does not hold a record" };
    return { ok: true, value };
  } catch {
    return { ok: false, reason: "the file is not valid JSON" };
  }
}

const seqWidth = 9;
const logName = /^(\d{9})-([0-9a-f]{64})\.qlog\.zst$/;
const checkpointName = /^(\d{9})-([0-9a-f]{64})\.qsnap\.zst$/;
const fileName = (seq: number, hash: string, kind: "qlog" | "qsnap") =>
  `${String(seq).padStart(seqWidth, "0")}-${hash}.${kind}.zst`;
// Strict names skip partial writes and sync-tool conflict copies such as
// "000000001-<hash> (1).qlog.zst".
export function parseName(name: string, kind: "qlog" | "qsnap") {
  const match = (kind === "qlog" ? logName : checkpointName).exec(name);
  return match ? { seq: Number(match[1]), hash: match[2] } : undefined;
}

export function encodeBatch(batch: Omit<Batch, "format"> & { seq: number }) {
  const value = { ...batch, format };
  const { hash, data } = pack(value);
  return { name: fileName(batch.seq, hash, "qlog"), hash, data };
}
export type Expected = { notebook: string; device: string };
export type Decoded<T> =
  | { ok: true; value: T; hash: string }
  | Damaged
  | { ok: false; newer: number; reason: string };
function header(
  value: Record<string, Json>,
  expected: Expected,
  seq: number,
): string | undefined {
  if (value.notebook !== expected.notebook)
    return "the file belongs to another notebook";
  if (value.device !== expected.device)
    return "the file is in another computer's folder";
  if (value.seq !== seq) return "the file's number does not match its name";
  if (!isCount(value.created)) return "the file has no creation time";
  return undefined;
}
const newer = (value: Record<string, Json>) =>
  typeof value.format === "number" && value.format > format
    ? {
        ok: false as const,
        newer: value.format,
        reason: `the file uses format ${value.format}; update Quire to read it`,
      }
    : undefined;
export function decodeBatch(
  name: string,
  data: Uint8Array,
  expected: Expected,
): Decoded<Batch> {
  const parsed = parseName(name, "qlog");
  if (!parsed) return { ok: false, reason: "the file name is not a batch" };
  const unpacked = unpack(data, parsed.hash);
  if (!unpacked.ok) return unpacked;
  const value = unpacked.value;
  const later = newer(value);
  if (later) return later;
  if (value.format !== format)
    return { ok: false, reason: "the file has an unknown format" };
  const problem = header(value, expected, parsed.seq);
  if (problem) return { ok: false, reason: problem };
  if (
    !(
      value.previous === null ||
      (isString(value.previous) && hex64.test(value.previous))
    ) ||
    (parsed.seq === 1) !== (value.previous === null)
  )
    return {
      ok: false,
      reason: "the file's link to the previous one is invalid",
    };
  if (
    !Array.isArray(value.attachments) ||
    !value.attachments.every((item) => isString(item) && !!attachmentHash(item))
  )
    return { ok: false, reason: "the file lists invalid attachments" };
  if (!Array.isArray(value.revisions))
    return { ok: false, reason: "the file has no revisions" };
  for (const revision of value.revisions) {
    const problem = revisionProblem(revision);
    if (problem) return { ok: false, reason: problem };
    if ((revision as Revision).device !== expected.device)
      return { ok: false, reason: "a revision names another computer" };
  }
  return { ok: true, value: value as unknown as Batch, hash: parsed.hash };
}

export function encodeCheckpoint(
  checkpoint: Omit<Checkpoint, "format"> & { seq: number },
) {
  const value = { ...checkpoint, format };
  const { hash, data } = pack(value);
  return { name: fileName(checkpoint.seq, hash, "qsnap"), hash, data };
}
export function decodeCheckpoint(
  name: string,
  data: Uint8Array,
  expected: Expected,
): Decoded<Checkpoint> {
  const parsed = parseName(name, "qsnap");
  if (!parsed)
    return { ok: false, reason: "the file name is not a checkpoint" };
  const unpacked = unpack(data, parsed.hash);
  if (!unpacked.ok) return unpacked;
  const value = unpacked.value;
  const later = newer(value);
  if (later) return later;
  if (value.format !== format)
    return { ok: false, reason: "the file has an unknown format" };
  const problem = header(value, expected, parsed.seq);
  if (problem) return { ok: false, reason: problem };
  const cursors = value.cursors;
  if (
    !cursors ||
    typeof cursors !== "object" ||
    Array.isArray(cursors) ||
    !Object.values(cursors).every(
      (cursor) =>
        !!cursor &&
        typeof cursor === "object" &&
        !Array.isArray(cursor) &&
        isCount(cursor.seq) &&
        isString(cursor.hash) &&
        hex64.test(cursor.hash),
    )
  )
    return { ok: false, reason: "the checkpoint's positions are invalid" };
  if (!Array.isArray(value.heads))
    return { ok: false, reason: "the checkpoint has no pages" };
  for (const revision of value.heads) {
    const problem = revisionProblem(revision);
    if (problem) return { ok: false, reason: problem };
  }
  return { ok: true, value: value as unknown as Checkpoint, hash: parsed.hash };
}

// Attachments are named by their content: "<sha256>.<ext>".
const attachmentName = /^([0-9a-f]{64})(\.[a-z0-9]{1,10})?$/;
export const attachmentHash = (name: string) => attachmentName.exec(name)?.[1];
export const attachmentFileName = (data: Uint8Array, original: string) => {
  const extension = /\.([A-Za-z0-9]{1,10})$/.exec(original)?.[1]?.toLowerCase();
  return sha256(data) + (extension ? "." + extension : "");
};

export function statusProblem(value: unknown): string | undefined {
  if (!value || typeof value !== "object") return "status is not an object";
  const s = value as Record<string, unknown>;
  return isCount(s.format) &&
    isString(s.app) &&
    isString(s.device) &&
    isString(s.label) &&
    isCount(s.lastSeq) &&
    isCount(s.outbox) &&
    isCount(s.heartbeat)
    ? undefined
    : "status is incomplete";
}
