import { DatabaseSync, type StatementSync } from "node:sqlite";
import zlib from "node:zlib";
import { canonical, type Cursor, type Revision } from "../../sync/format";
import type { ReplicaStore } from "../../sync/replica";

// This computer's copy of a Quire notebook. It is a cache of the synced
// log plus the outbox of saves not yet published, and can be rebuilt by
// deleting it. Saves use WAL with synchronous=FULL, so a revision reported
// as saved survives a crash or power loss.
const schema = `
  CREATE TABLE IF NOT EXISTS revisions (
    rowid INTEGER PRIMARY KEY,
    id TEXT NOT NULL UNIQUE,
    page TEXT NOT NULL,
    created INTEGER NOT NULL,
    body BLOB NOT NULL
  );
  CREATE INDEX IF NOT EXISTS revisions_by_page ON revisions(page, rowid);
  CREATE TABLE IF NOT EXISTS heads (
    page TEXT NOT NULL,
    rank INTEGER NOT NULL,
    revision TEXT NOT NULL,
    PRIMARY KEY (page, rank)
  );
  CREATE TABLE IF NOT EXISTS pages (
    id TEXT PRIMARY KEY,
    path TEXT NOT NULL,
    key TEXT NOT NULL,
    head TEXT NOT NULL,
    text TEXT NOT NULL,
    deleted INTEGER NOT NULL,
    conflicted INTEGER NOT NULL,
    modified INTEGER NOT NULL
  );
  CREATE INDEX IF NOT EXISTS pages_by_key ON pages(key);
  CREATE TABLE IF NOT EXISTS cursors (
    device TEXT PRIMARY KEY,
    seq INTEGER NOT NULL,
    hash TEXT NOT NULL
  );
  CREATE TABLE IF NOT EXISTS outbox (
    position INTEGER PRIMARY KEY,
    revision TEXT NOT NULL UNIQUE
  );
  CREATE TABLE IF NOT EXISTS recovery (
    path TEXT PRIMARY KEY,
    text TEXT NOT NULL,
    expected TEXT,
    saved_at INTEGER NOT NULL
  );
  CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
`;
export const schemaVersion = 1;
const pageColumns = "id,path,head,text,deleted,conflicted,modified";
export const pathKey = (path: string) => path.normalize("NFC").toLowerCase();

export type PageRow = {
  id: string;
  path: string;
  head: string;
  text: string;
  deleted: number;
  conflicted: number;
  modified: number;
};

export class SqliteReplicaStore implements ReplicaStore {
  readonly db: DatabaseSync;
  private depth = 0;
  private cache = new Map<string, Revision>();
  private statements: Record<string, StatementSync>;
  constructor(file: string) {
    this.db = new DatabaseSync(file);
    this.db.exec(
      "PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA busy_timeout=5000; PRAGMA foreign_keys=ON;",
    );
    const version = Number(
      this.db.prepare("PRAGMA user_version").get()?.user_version ?? 0,
    );
    if (version > schemaVersion)
      throw new Error(
        "This notebook's local database was made by a newer Quire. Update Quire, or delete the database to rebuild it from the synced folder.",
      );
    this.db.exec(schema);
    // Added within schema version 1, so older Quire versions still open
    // the database; they leave it at 0.
    if (
      !(
        this.db.prepare("PRAGMA table_info(recovery)").all() as {
          name: string;
        }[]
      ).some((column) => column.name === "base_known")
    )
      this.db.exec(
        "ALTER TABLE recovery ADD COLUMN base_known INTEGER NOT NULL DEFAULT 0",
      );
    this.db.exec(`PRAGMA user_version=${schemaVersion}`);
    const prepare = (sql: string) => this.db.prepare(sql);
    this.statements = {
      get: prepare("SELECT body FROM revisions WHERE id=?"),
      insert: prepare(
        "INSERT OR IGNORE INTO revisions(id,page,created,body) VALUES (?,?,?,?)",
      ),
      heads: prepare("SELECT revision FROM heads WHERE page=? ORDER BY rank"),
      clearHeads: prepare("DELETE FROM heads WHERE page=?"),
      addHead: prepare("INSERT INTO heads(page,rank,revision) VALUES (?,?,?)"),
      page: prepare(
        `INSERT INTO pages(id,path,key,head,text,deleted,conflicted,modified) VALUES (?,?,?,?,?,?,?,?)
         ON CONFLICT(id) DO UPDATE SET path=excluded.path,key=excluded.key,head=excluded.head,text=excluded.text,deleted=excluded.deleted,conflicted=excluded.conflicted,modified=excluded.modified`,
      ),
      pages: prepare("SELECT id FROM pages ORDER BY id"),
      cursor: prepare("SELECT seq,hash FROM cursors WHERE device=?"),
      setCursor: prepare(
        "INSERT INTO cursors(device,seq,hash) VALUES (?,?,?) ON CONFLICT(device) DO UPDATE SET seq=excluded.seq,hash=excluded.hash",
      ),
      outbox: prepare("SELECT revision FROM outbox ORDER BY position"),
      addOutbox: prepare("INSERT OR IGNORE INTO outbox(revision) VALUES (?)"),
      removeOutbox: prepare("DELETE FROM outbox WHERE revision=?"),
    };
  }
  close() {
    this.db.close();
  }

  get(id: string) {
    const cached = this.cache.get(id);
    if (cached) return cached;
    const row = this.statements.get.get(id) as { body: Uint8Array } | undefined;
    if (!row) return undefined;
    const revision = JSON.parse(
      zlib.zstdDecompressSync(row.body).toString("utf8"),
    ) as Revision;
    if (this.cache.size > 20_000) this.cache.clear();
    this.cache.set(id, revision);
    return revision;
  }
  heads(page: string) {
    return (this.statements.heads.all(page) as { revision: string }[]).map(
      (row) => row.revision,
    );
  }
  put(revision: Revision, heads: string[]) {
    this.transaction(() => {
      this.statements.insert.run(
        revision.id,
        revision.page,
        revision.created,
        zlib.zstdCompressSync(Buffer.from(canonical(revision), "utf8")),
      );
      this.cache.set(revision.id, revision);
      this.statements.clearHeads.run(revision.page);
      heads.forEach((id, rank) =>
        this.statements.addHead.run(revision.page, rank, id),
      );
      const primary = this.get(heads[0])!;
      this.statements.page.run(
        revision.page,
        primary.path,
        pathKey(primary.path),
        primary.id,
        primary.text,
        primary.deleted ? 1 : 0,
        heads.length > 1 ? 1 : 0,
        primary.created,
      );
    });
  }
  pages() {
    return (this.statements.pages.all() as { id: string }[]).map((r) => r.id);
  }
  cursor(device: string) {
    const row = this.statements.cursor.get(device) as Cursor | undefined;
    return row ? { seq: Number(row.seq), hash: String(row.hash) } : undefined;
  }
  setCursor(device: string, cursor: Cursor) {
    this.statements.setCursor.run(device, cursor.seq, cursor.hash);
  }
  outbox() {
    return (this.statements.outbox.all() as { revision: string }[]).map(
      (row) => row.revision,
    );
  }
  addOutbox(id: string) {
    this.statements.addOutbox.run(id);
  }
  inOutbox(id: string) {
    return !!this.db.prepare("SELECT 1 FROM outbox WHERE revision=?").get(id);
  }
  discard(id: string) {
    const revision = this.get(id);
    if (!revision) return;
    this.transaction(() => {
      this.db.prepare("DELETE FROM revisions WHERE id=?").run(id);
      this.db.prepare("DELETE FROM outbox WHERE revision=?").run(id);
      const heads = this.heads(revision.page).filter((head) => head !== id);
      this.statements.clearHeads.run(revision.page);
      heads.forEach((head, rank) =>
        this.statements.addHead.run(revision.page, rank, head),
      );
    });
    this.cache.delete(id);
  }
  removeOutbox(ids: string[]) {
    this.transaction(() => {
      for (const id of ids) this.statements.removeOutbox.run(id);
    });
  }
  transaction<T>(run: () => T): T {
    if (this.depth) {
      this.depth++;
      try {
        return run();
      } finally {
        this.depth--;
      }
    }
    this.db.exec("BEGIN IMMEDIATE");
    this.depth = 1;
    try {
      const result = run();
      this.db.exec("COMMIT");
      return result;
    } catch (error) {
      this.db.exec("ROLLBACK");
      this.cache.clear();
      throw error;
    } finally {
      this.depth = 0;
    }
  }

  // Live pages: the current version of every page that is not deleted,
  // by path. Sorting here rather than in SQLite avoids moving every
  // page's text through SQLite's sorter, which took 190 ms at 32,000 pages.
  livePages() {
    return (
      this.db
        .prepare(
          `SELECT ${pageColumns} FROM pages WHERE deleted=0 AND id NOT LIKE 's-%'`,
        )
        .all() as PageRow[]
    ).sort((a, b) => (a.path < b.path ? -1 : a.path > b.path ? 1 : 0));
  }
  // Where each page is and which version it is at, without its text.
  pageVersions() {
    return this.db
      .prepare("SELECT id,path,head,deleted FROM pages WHERE id NOT LIKE 's-%'")
      .all() as Pick<PageRow, "id" | "path" | "head" | "deleted">[];
  }
  pageByPath(path: string) {
    return this.db
      .prepare(
        `SELECT ${pageColumns} FROM pages WHERE key=? AND deleted=0 AND id NOT LIKE 's-%' ORDER BY id LIMIT 1`,
      )
      .get(pathKey(path)) as PageRow | undefined;
  }
  pageById(id: string) {
    return this.db
      .prepare(`SELECT ${pageColumns} FROM pages WHERE id=?`)
      .get(id) as PageRow | undefined;
  }
  // Every saved version of a page, newest first.
  revisionsOf(page: string) {
    return (
      this.db
        .prepare(
          "SELECT rowid,id,created FROM revisions WHERE page=? ORDER BY rowid DESC",
        )
        .all(page) as { rowid: number; id: string; created: number }[]
    ).map((row) => ({
      rowid: Number(row.rowid),
      id: String(row.id),
      created: Number(row.created),
    }));
  }
  revisionByRow(rowid: number) {
    const row = this.db
      .prepare("SELECT id FROM revisions WHERE rowid=?")
      .get(rowid) as { id: string } | undefined;
    return row ? this.get(row.id) : undefined;
  }
  meta(key: string) {
    return (
      this.db.prepare("SELECT value FROM meta WHERE key=?").get(key) as
        { value: string } | undefined
    )?.value;
  }
  setMeta(key: string, value: string) {
    this.db
      .prepare(
        "INSERT INTO meta(key,value) VALUES (?,?) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
      )
      .run(key, value);
  }
  recovery() {
    return (
      this.db
        .prepare("SELECT path,text,expected,saved_at,base_known FROM recovery")
        .all() as {
        path: string;
        text: string;
        expected: string | null;
        saved_at: number;
        base_known: number;
      }[]
    ).map((row) => ({
      path: row.path,
      text: row.text,
      expectedRevision: row.expected,
      savedAt: Number(row.saved_at),
      baseKnown: Number(row.base_known) === 1,
    }));
  }
  recordRecovery(draft: {
    path: string;
    text: string;
    expectedRevision: string | null;
    savedAt: number;
    baseKnown?: boolean;
  }) {
    this.db
      .prepare(
        "INSERT INTO recovery(path,text,expected,saved_at,base_known) VALUES (?,?,?,?,?) ON CONFLICT(path) DO UPDATE SET text=excluded.text,expected=excluded.expected,saved_at=excluded.saved_at,base_known=excluded.base_known WHERE excluded.saved_at>=recovery.saved_at",
      )
      .run(
        draft.path,
        draft.text,
        draft.expectedRevision,
        draft.savedAt,
        draft.baseKnown ? 1 : 0,
      );
  }
  clearRecovery(path: string, onlyText?: string) {
    if (onlyText === undefined)
      this.db.prepare("DELETE FROM recovery WHERE path=?").run(path);
    else
      this.db
        .prepare("DELETE FROM recovery WHERE path=? AND text=?")
        .run(path, onlyText);
  }
  integrity() {
    const row = this.db.prepare("PRAGMA quick_check").get() as
      Record<string, unknown> | undefined;
    return Object.values(row ?? {})[0] === "ok";
  }
}
