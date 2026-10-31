import { mkdirSync } from "node:fs";
import path from "node:path";
import { DatabaseSync } from "node:sqlite";
import type { DataProfile, DataState, ProfileId } from "../core/types";

export const overlaps = (left: string, right: string) => {
  const a = path.resolve(left).toLowerCase();
  const b = path.resolve(right).toLowerCase();
  const inside = (parent: string, child: string) => {
    const relative = path.relative(parent, child);
    return (
      !path.isAbsolute(relative) &&
      relative !== ".." &&
      !relative.startsWith(`..${path.sep}`)
    );
  };
  return inside(a, b) || inside(b, a);
};

export class Profiles {
  private db: DatabaseSync;
  constructor(directory: string) {
    mkdirSync(directory, { recursive: true });
    this.db = new DatabaseSync(path.join(directory, "profiles.sqlite"));
    this.db.exec(`
      PRAGMA journal_mode=WAL;
      PRAGMA synchronous=FULL;
      CREATE TABLE IF NOT EXISTS profiles (
        id TEXT PRIMARY KEY, label TEXT NOT NULL, root TEXT,
        backupDirectory TEXT, lastBackupAt INTEGER, lastBackupPath TEXT, backupError TEXT,
        gitRemote TEXT, lastPushAt INTEGER, gitError TEXT
      );
      CREATE TABLE IF NOT EXISTS preferences (key TEXT PRIMARY KEY, value TEXT NOT NULL);
      INSERT OR IGNORE INTO profiles(id,label) VALUES ('current','Current'),('work','Work'),('personal','Personal');
      INSERT OR IGNORE INTO preferences(key,value) VALUES ('active','current');
    `);
  }
  close() {
    this.db.close();
  }
  state(): DataState {
    const active = this.db
      .prepare("SELECT value FROM preferences WHERE key='active'")
      .get() as { value: ProfileId };
    return {
      active: active.value,
      profiles: this.db
        .prepare("SELECT * FROM profiles ORDER BY rowid")
        .all() as DataProfile[],
    };
  }
  get(id: ProfileId) {
    const profile = this.db
      .prepare("SELECT * FROM profiles WHERE id=?")
      .get(id) as DataProfile | undefined;
    if (!profile)
      throw new Error(
        "That profile does not exist. Select Current, Work, or Personal.",
      );
    return profile;
  }
  active() {
    return this.get(this.state().active);
  }
  activate(id: ProfileId) {
    this.get(id);
    this.db
      .prepare("UPDATE preferences SET value=? WHERE key='active'")
      .run(id);
  }
  migrate(root: string | null) {
    if (
      this.db
        .prepare("SELECT value FROM preferences WHERE key='migrated'")
        .get()
    )
      return;
    this.db.exec("BEGIN IMMEDIATE");
    try {
      if (root)
        this.db
          .prepare("UPDATE profiles SET root=? WHERE id='current'")
          .run(root);
      this.db.exec(
        "INSERT INTO preferences(key,value) VALUES ('migrated','1'); COMMIT;",
      );
    } catch (error) {
      this.db.exec("ROLLBACK");
      throw error;
    }
  }
  validateRoot(id: ProfileId, root: string) {
    this.get(id);
    const others = this.state().profiles.filter((p) => p.id !== id && p.root);
    const transferred = others.find(
      (p) =>
        p.id === "current" &&
        p.root?.toLowerCase() === root.toLowerCase() &&
        id !== "current",
    );
    for (const other of others) {
      if (other !== transferred && overlaps(root, other.root!))
        throw new Error(
          `This folder overlaps the ${other.label} notebook. Choose a separate folder for ${this.get(id).label}.`,
        );
    }
    for (const profile of this.state().profiles)
      if (profile.backupDirectory && overlaps(root, profile.backupDirectory))
        throw new Error(
          "The notebook overlaps a backup destination. Choose separate folders for live notes and backups.",
        );
    return transferred;
  }
  setRoot(id: ProfileId, root: string) {
    const transferred = this.validateRoot(id, root);
    this.db.exec("BEGIN IMMEDIATE");
    try {
      if (transferred)
        this.db
          .prepare("UPDATE profiles SET root=NULL WHERE id='current'")
          .run();
      this.db
        .prepare(
          "UPDATE profiles SET root=?,lastBackupAt=NULL,lastBackupPath=NULL,backupError=NULL,gitRemote=NULL,lastPushAt=NULL,gitError=NULL WHERE id=?",
        )
        .run(root, id);
      this.db.exec("COMMIT");
    } catch (error) {
      this.db.exec("ROLLBACK");
      throw error;
    }
  }
  setBackupDirectory(id: ProfileId, directory: string) {
    for (const profile of this.state().profiles)
      if (profile.root && overlaps(directory, profile.root))
        throw new Error(
          "The backup destination overlaps a live notebook. Choose an independent folder or external drive.",
        );
    this.db
      .prepare(
        "UPDATE profiles SET backupDirectory=?,lastBackupAt=NULL,lastBackupPath=NULL,backupError=NULL WHERE id=?",
      )
      .run(directory, id);
  }
  setGitRemote(id: ProfileId, remote: string) {
    if (id !== "personal")
      throw new Error(
        "Git sync is available only in Personal. Select the Personal profile first.",
      );
    const value = remote.trim();
    if (
      value &&
      (value.startsWith("-") ||
        /[\r\n\0]/.test(value) ||
        /^https?:\/\/[^/]*@/i.test(value))
    )
      throw new Error(
        "The Git remote is invalid or contains credentials. Use a remote URL with credentials managed by Git.",
      );
    this.db
      .prepare(
        "UPDATE profiles SET gitRemote=?,lastPushAt=NULL,gitError=NULL WHERE id=?",
      )
      .run(value || null, id);
  }
  backupResult(
    id: ProfileId,
    result: { at: number; path: string } | { error: string },
  ) {
    if ("error" in result)
      this.db
        .prepare("UPDATE profiles SET backupError=? WHERE id=?")
        .run(result.error, id);
    else
      this.db
        .prepare(
          "UPDATE profiles SET lastBackupAt=?,lastBackupPath=?,backupError=NULL WHERE id=?",
        )
        .run(result.at, result.path, id);
  }
  gitResult(id: ProfileId, result: { at: number } | { error: string }) {
    if ("error" in result)
      this.db
        .prepare("UPDATE profiles SET gitError=? WHERE id=?")
        .run(result.error, id);
    else
      this.db
        .prepare("UPDATE profiles SET lastPushAt=?,gitError=NULL WHERE id=?")
        .run(result.at, id);
  }
}
