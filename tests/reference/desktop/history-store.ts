import { createHash } from "node:crypto";
import zlib from "node:zlib";
import type { DatabaseSync } from "node:sqlite";

// Saved page history. Each distinct text is stored once in `contents`,
// keyed by its SHA-256 and compressed with zstd when that is smaller;
// `history` rows point at it. Older rows thin out by age (see prune).
export type HistoryRow = {
  path: string;
  text: string;
  revision: string;
  saved_at: number;
};
const hour = 3_600_000,
  day = 24 * hour;
// Every save from the last day, the newest per hour for 30 days, then the
// newest per (UTC) day.
const retention = { all: day, hourly: 30 * day };

const schema = `
  CREATE TABLE IF NOT EXISTS contents (
    hash TEXT PRIMARY KEY,
    codec TEXT NOT NULL,
    data BLOB NOT NULL
  );
  CREATE TABLE IF NOT EXISTS history (
    id INTEGER PRIMARY KEY,
    path TEXT NOT NULL,
    content TEXT NOT NULL,
    revision TEXT NOT NULL,
    saved_at INTEGER NOT NULL
  );
  CREATE INDEX IF NOT EXISTS history_by_path ON history(path, id DESC);
  CREATE INDEX IF NOT EXISTS history_by_content ON history(content);
`;
const hasTable = (db: DatabaseSync, name: string) =>
  !!db
    .prepare("SELECT 1 FROM sqlite_master WHERE type='table' AND name=?")
    .get(name);

function encode(text: string): { codec: string; data: Uint8Array } {
  const raw = Buffer.from(text, "utf8"),
    packed = zlib.zstdCompressSync(raw);
  return packed.length < raw.length
    ? { codec: "zstd", data: packed }
    : { codec: "raw", data: raw };
}
function decode(codec: string, data: Uint8Array) {
  if (codec === "raw") return Buffer.from(data).toString("utf8");
  if (codec === "zstd") return zlib.zstdDecompressSync(data).toString("utf8");
  throw new Error(
    `A saved revision uses the ${codec} format, which this version cannot read. Update Quire and reopen page history.`,
  );
}
function putContent(db: DatabaseSync, text: string) {
  const hash = createHash("sha256").update(text).digest("hex");
  if (!db.prepare("SELECT 1 FROM contents WHERE hash=?").get(hash)) {
    const { codec, data } = encode(text);
    db.prepare("INSERT INTO contents(hash,codec,data) VALUES (?,?,?)").run(
      hash,
      codec,
      data,
    );
  }
  return hash;
}
function insert(db: DatabaseSync, row: HistoryRow, id?: number) {
  db.prepare(
    "INSERT INTO history(id,path,content,revision,saved_at) VALUES (?,?,?,?,?)",
  ).run(
    id ?? null,
    row.path,
    putContent(db, row.text),
    row.revision,
    row.saved_at,
  );
}
function inTransaction(db: DatabaseSync, run: () => void) {
  if (db.isTransaction) return run();
  db.exec("BEGIN IMMEDIATE");
  try {
    run();
    db.exec("COMMIT");
  } catch (error) {
    db.exec("ROLLBACK");
    throw error;
  }
}

// Creates the tables and moves rows from the old one-text-per-row
// `revisions` table, keeping their ids so the order survives.
export function openHistory(db: DatabaseSync) {
  db.exec(schema);
  if (!hasTable(db, "revisions")) return;
  inTransaction(db, () => {
    // Row by row, so a large history never sits in memory at once.
    for (const row of db
      .prepare(
        "SELECT id,path,text,revision,saved_at FROM revisions ORDER BY id",
      )
      .iterate() as Iterable<HistoryRow & { id: number }>)
      insert(db, row, row.id);
    db.exec("DROP TABLE revisions");
    const now = Date.now();
    for (const { path } of db
      .prepare("SELECT DISTINCT path FROM history")
      .all() as { path: string }[])
      prune(db, path, now);
  });
  // Returns the old table's space to the disk once.
  db.exec("VACUUM");
}
export function addRevision(db: DatabaseSync, row: HistoryRow) {
  inTransaction(db, () => {
    insert(db, row);
    prune(db, row.path, Date.now());
  });
}
// Every row with its text, from this format or the old `revisions` table,
// such as another device's older snapshot.
export function readHistory(db: DatabaseSync): HistoryRow[] {
  if (hasTable(db, "history"))
    return (
      db
        .prepare(
          "SELECT h.path,c.codec,c.data,h.revision,h.saved_at FROM history h JOIN contents c ON c.hash=h.content ORDER BY h.id",
        )
        .all() as {
        path: string;
        codec: string;
        data: Uint8Array;
        revision: string;
        saved_at: number;
      }[]
    ).map((row) => ({
      path: row.path,
      text: decode(row.codec, row.data),
      revision: row.revision,
      saved_at: row.saved_at,
    }));
  if (hasTable(db, "revisions"))
    return db
      .prepare("SELECT path,text,revision,saved_at FROM revisions")
      .all() as HistoryRow[];
  return [];
}
export const listRevisions = (db: DatabaseSync, path: string) =>
  db
    .prepare(
      "SELECT id, revision, saved_at AS savedAt FROM history WHERE path=? ORDER BY id DESC",
    )
    .all(path) as { id: number; revision: string; savedAt: number }[];
export function revisionText(db: DatabaseSync, path: string, id: number) {
  const row = db
    .prepare(
      "SELECT c.codec,c.data FROM history h JOIN contents c ON c.hash=h.content WHERE h.path=? AND h.id=?",
    )
    .get(path, id) as { codec: string; data: Uint8Array } | undefined;
  return row && decode(row.codec, row.data);
}
export const hasRevision = (db: DatabaseSync, path: string, revision: string) =>
  !!db
    .prepare("SELECT 1 FROM history WHERE path=? AND revision=?")
    .get(path, revision);
// Gives a moved page the history of its old path, skipping rows it has.
export function copyHistory(db: DatabaseSync, from: string, to: string) {
  db.prepare(
    "INSERT INTO history(path,content,revision,saved_at) SELECT ?,content,revision,saved_at FROM history AS old WHERE path=? AND NOT EXISTS(SELECT 1 FROM history AS dest WHERE dest.path=? AND dest.revision=old.revision AND dest.saved_at=old.saved_at)",
  ).run(to, from, to);
}
// Keeps the newest row in each bucket: one per row inside a day, one per
// hour inside 30 days, one per UTC day after that. Texts no row points at
// any more are deleted with them.
function prune(db: DatabaseSync, path: string, now: number) {
  const released = db
    .prepare(
      `DELETE FROM history WHERE path=? AND id NOT IN (
      SELECT id FROM (
        SELECT id, ROW_NUMBER() OVER (PARTITION BY
          CASE WHEN saved_at >= ? THEN 'r' || id
               WHEN saved_at >= ? THEN 'h' || (saved_at / ${hour})
               ELSE 'd' || (saved_at / ${day}) END
          ORDER BY saved_at DESC, id DESC) AS rank
        FROM history WHERE path=?
      ) WHERE rank = 1
    ) RETURNING content`,
    )
    .all(path, now - retention.all, now - retention.hourly, path) as {
    content: string;
  }[];
  const drop = db.prepare(
    "DELETE FROM contents WHERE hash=? AND NOT EXISTS (SELECT 1 FROM history WHERE content=?)",
  );
  for (const hash of new Set(released.map((row) => row.content)))
    drop.run(hash, hash);
}
