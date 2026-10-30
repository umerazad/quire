// Copied from rt src/desktop/search-index.ts at 5ea5c22310ff (unchanged since 5d3a04a9b54a).
// Only import paths changed; SearchHit is inlined from src/core/types.ts.
import { DatabaseSync, type StatementSync } from "node:sqlite";
import { createHash } from "node:crypto";
import { pageTitle, parseBlocks } from "../markdown/markdown";
export type SearchHit = { path: string; title: string; snippet: string; blockId?: string };

type SearchDocument = { path: string; text: string; revision?: string };
const fingerprint = (text: string) =>
  createHash("sha256").update(text).digest("hex");

export class SearchIndex {
  private db: DatabaseSync;
  private insertBlock: StatementSync;
  private deleteBlocks: StatementSync;
  private writePage: StatementSync;
  private deletePage: StatementSync;
  private readPage: StatementSync;
  constructor(file: string) {
    this.db = new DatabaseSync(file);
    this.db.exec(
      "PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA busy_timeout=5000;",
    );
    const version = this.db.prepare("PRAGMA user_version").get()?.user_version;
    if (version !== 2) {
      this.db.exec(`
        BEGIN;
        DROP TABLE IF EXISTS notes;
        DROP TABLE IF EXISTS blocks;
        DROP TABLE IF EXISTS pages;
        CREATE TABLE pages (path TEXT PRIMARY KEY, revision TEXT NOT NULL);
        CREATE TABLE blocks (rowid INTEGER PRIMARY KEY, path TEXT NOT NULL, title TEXT NOT NULL, text TEXT NOT NULL, blockId TEXT);
        CREATE INDEX blocks_path ON blocks(path);
        CREATE VIRTUAL TABLE notes USING fts5(path UNINDEXED, title, text, blockId UNINDEXED, content='blocks', content_rowid='rowid', tokenize='unicode61');
        CREATE TRIGGER blocks_insert AFTER INSERT ON blocks BEGIN
          INSERT INTO notes(rowid,path,title,text,blockId) VALUES (new.rowid,new.path,new.title,new.text,new.blockId);
        END;
        CREATE TRIGGER blocks_delete AFTER DELETE ON blocks BEGIN
          INSERT INTO notes(notes,rowid,path,title,text,blockId) VALUES ('delete',old.rowid,old.path,old.title,old.text,old.blockId);
        END;
        PRAGMA user_version=2;
        COMMIT;
      `);
    }
    this.insertBlock = this.db.prepare(
      "INSERT INTO blocks(path,title,text,blockId) VALUES (?,?,?,?)",
    );
    this.deleteBlocks = this.db.prepare("DELETE FROM blocks WHERE path=?");
    this.writePage = this.db.prepare(
      "INSERT OR REPLACE INTO pages(path,revision) VALUES (?,?)",
    );
    this.deletePage = this.db.prepare("DELETE FROM pages WHERE path=?");
    this.readPage = this.db.prepare("SELECT revision FROM pages WHERE path=?");
  }
  private transaction<T>(operation: () => T): T {
    this.db.exec("BEGIN");
    try {
      const result = operation();
      this.db.exec("COMMIT");
      return result;
    } catch (error) {
      this.db.exec("ROLLBACK");
      throw error;
    }
  }
  private put(path: string, text: string, revision: string) {
    this.deleteBlocks.run(path);
    const title = pageTitle(path, text);
    this.insertBlock.run(path, title, "", null);
    for (const block of parseBlocks(text))
      this.insertBlock.run(path, title, block.text, block.id ?? null);
    this.writePage.run(path, revision);
  }
  update(path: string, text: string) {
    const revision = fingerprint(text);
    if (this.readPage.get(path)?.revision === revision) return;
    this.transaction(() => this.put(path, text, revision));
  }
  reconcile(documents: readonly SearchDocument[]) {
    const existing = new Map(
      this.db
        .prepare("SELECT path,revision FROM pages")
        .all()
        .map((row) => [String(row.path), String(row.revision)]),
    );
    const present = new Set<string>();
    const result = { updated: 0, removed: 0, unchanged: 0 };
    return this.transaction(() => {
      for (const doc of documents) {
        if (present.has(doc.path))
          throw new Error(
            "Search received duplicate page paths. Rescan the notebook before rebuilding search.",
          );
        present.add(doc.path);
        const revision = doc.revision ?? fingerprint(doc.text);
        if (existing.get(doc.path) === revision) result.unchanged++;
        else {
          this.put(doc.path, doc.text, revision);
          result.updated++;
        }
      }
      for (const name of existing.keys()) {
        if (present.has(name)) continue;
        this.deleteBlocks.run(name);
        this.deletePage.run(name);
        result.removed++;
      }
      return result;
    });
  }
  remove(path: string) {
    this.transaction(() => {
      this.deleteBlocks.run(path);
      this.deletePage.run(path);
    });
  }
  reset() {
    this.transaction(() => {
      this.db.exec("DELETE FROM blocks; DELETE FROM pages;");
    });
  }
  search(query: string): SearchHit[] {
    const terms = query.match(/[\p{L}\p{N}_-]+/gu) ?? [];
    if (!terms.length) return [];
    const expression = terms.map((term) => `"${term}"*`).join(" AND ");
    return this.db
      .prepare(
        "SELECT path,title,snippet(notes,2,'','',' … ',24) AS snippet,blockId FROM notes WHERE notes MATCH ? ORDER BY bm25(notes) LIMIT 40",
      )
      .all(expression) as unknown as SearchHit[];
  }
  close() {
    this.db.close();
  }
}
