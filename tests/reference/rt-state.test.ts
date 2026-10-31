// The synthetic rt installation in tests/fixtures/rt-state, read back through rt's own frozen
// desktop code on a copy, so Rust migration tests start from state rt itself accepts.
import { expect, it } from "vitest";
import { cpSync, mkdtempSync, readFileSync, realpathSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { DatabaseSync } from "node:sqlite";
import { Profiles } from "./desktop/profiles";
import { VaultFiles } from "./desktop/vault";
import { NotebookEngine } from "./desktop/engine/engine";

const fixture = path.resolve("tests/fixtures/rt-state");
const copy = () => {
  const root = realpathSync(mkdtempSync(path.join(tmpdir(), "rt-state-test-")));
  cpSync(fixture, root, { recursive: true });
  return root;
};

it("keeps three profiles with the placeholder roots, Work active", () => {
  const profiles = new Profiles(path.join(copy(), "app-data"));
  const state = profiles.state();
  profiles.close();
  expect(state.active).toBe("work");
  expect(state.profiles.map((p) => [p.id, p.root, p.backupDirectory, p.gitRemote])).toEqual([
    ["current", "/rt-fixture/folder-notebook", "/rt-fixture/backups", null],
    ["work", "/rt-fixture/quire-notebook", "/rt-fixture/backups", null],
    ["personal", "/rt-fixture/pending-notebook", null, "git@example.com:notes.git"],
  ]);
});

it("opens the folder notebook with its draft and saved history", async () => {
  const root = copy();
  const files = new VaultFiles(path.join(root, "folder-notebook"), path.join(root, "app-data"));
  const snapshot = await files.snapshot();
  expect(snapshot.documents.map((d) => d.path).sort()).toEqual(["Ideas.md", "Plans.md", "Projects/Garden.md"]);
  expect(snapshot.recovery?.map((r) => [r.path, r.text])).toEqual([["Ideas.md", "- مرحبا 🙂 ideas, unsaved\n"]]);
  expect(files.revisions("Plans.md")).toHaveLength(1);
  files.close();
});

const localDatabase = (root: string, notebook: string) => {
  const { id } = JSON.parse(readFileSync(path.join(root, notebook, "quire-notebook.json"), "utf8"));
  return new DatabaseSync(path.join(root, "app-data", "notebooks", id, "notebook.sqlite"), { readOnly: true });
};

it("keeps both Quire notebooks' drafts; one still holds a save it never published", () => {
  const root = copy();
  const work = localDatabase(root, "quire-notebook");
  expect(work.prepare("SELECT path,base_known FROM recovery ORDER BY path").all()).toEqual([
    { path: "Tasks/Today.md", base_known: 0 },
    { path: "Welcome.md", base_known: 1 },
  ]);
  expect(work.prepare("SELECT COUNT(*) AS n FROM outbox").get()).toEqual({ n: 0 });
  work.close();
  const pending = localDatabase(root, "pending-notebook");
  expect(pending.prepare("SELECT COUNT(*) AS n FROM outbox").get()).toEqual({ n: 1 });
  pending.close();
});

it("settles the draft typed into a known version when rt opens the notebook", async () => {
  const root = copy();
  const engine = await NotebookEngine.open({ folder: path.join(root, "quire-notebook"), appData: path.join(root, "app-data") });
  const pages = Object.fromEntries(engine.snapshot().documents.map((d) => [d.path, d.text]));
  expect(pages["Welcome.md"]).toBe("- Welcome, typed but not saved\n");
  expect(engine.store.recovery().map((r) => r.path)).toEqual(["Tasks/Today.md"]);
  engine.store.close();
});
