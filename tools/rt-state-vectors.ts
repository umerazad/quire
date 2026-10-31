// Writes a synthetic rt 0.9.57 installation into tests/fixtures/rt-state through rt's own
// writers, frozen in tests/reference/desktop: its application data (profiles.sqlite,
// settings.json, appearance.json, ai-settings.json, device-id, a Quire notebook's local
// database), a folder notebook with drafts and history in .quire, two Quire notebook synced
// folders, and one backup of each kind. Absolute roots in profiles.sqlite and settings.json use
// the placeholder prefix /rt-fixture; tests copy the tree and point the prefix at the copy.
// Run with `bun tools/rt-state-vectors.ts`.
import { cpSync, mkdirSync, mkdtempSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { Profiles } from "../tests/reference/desktop/profiles.ts";
import { VaultFiles, revision } from "../tests/reference/desktop/vault.ts";
import { Backups } from "../tests/reference/desktop/backups.ts";
import { NotebookEngine, createQuireNotebook } from "../tests/reference/desktop/engine/engine.ts";
import { EngineFiles } from "../tests/reference/desktop/engine/engine-files.ts";

const placeholder = "/rt-fixture";
const output = path.resolve("tests/fixtures/rt-state");
const work = realpathSync(mkdtempSync(path.join(tmpdir(), "rt-state-")));
const appData = path.join(work, "app-data");
const folderRoot = path.join(work, "folder-notebook");
const quireRoot = path.join(work, "quire-notebook");
const pendingRoot = path.join(work, "pending-notebook");
const backupRoot = path.join(work, "backups");
const source = path.join(work, "photo.png");
for (const folder of [appData, folderRoot, backupRoot]) mkdirSync(folder, { recursive: true });
writeFileSync(source, Uint8Array.from([0x89, 0x50, 0x4e, 0x47, 0, 1, 2, 3]));
const clock = (() => {
  let now = Date.UTC(2026, 9, 1, 9, 0, 0);
  return () => (now += 1000);
})();

// A folder notebook: notes, an attachment, a saved history, a draft, and its layout settings.
const folder = new VaultFiles(folderRoot, appData);
const first = "# Plans\n\n- Draft the migration [[Ideas]]\n";
await folder.save("Plans.md", first, null);
await folder.save("Ideas.md", "- مرحبا 🙂 ideas\n", null);
await folder.save("Projects/Garden.md", "- [ ] Plant tulips\n", null);
const attachment = await folder.attach(source);
const second = `${first}- Photo ![](${attachment})\n`;
await folder.save("Plans.md", second, revision(first));
await folder.recover({
  path: "Ideas.md",
  text: "- مرحبا 🙂 ideas, unsaved\n",
  expectedRevision: revision("- مرحبا 🙂 ideas\n"),
  savedAt: clock(),
});
await folder.writeNotebookSettings(
  JSON.stringify({ version: 1, daily: { folder: "Journal", title: "{YYYY}-{MM}-{DD}" } }, null, 2) + "\n",
);
await folder.publishHistorySnapshot();

// A Quire notebook with two published saves, an attachment, and two drafts: one typed into a
// known version, one from a build that did not record which version it was typed into.
await createQuireNotebook(quireRoot);
const engine = await NotebookEngine.open({ folder: quireRoot, appData, label: "office", app: "0.9.57", now: clock });
const quire = new EngineFiles(engine);
const welcome = await quire.save("Welcome.md", "- Welcome to the Quire notebook\n", null);
const image = await quire.attach(source);
const later = await quire.save("Welcome.md", `- Welcome to the Quire notebook\n- ![](${image})\n`, welcome.ok ? welcome.revision : null);
await quire.save("Tasks/Today.md", "- [ ] Call back\n", null);
await quire.recover({
  path: "Welcome.md",
  text: "- Welcome, typed but not saved\n",
  expectedRevision: later.ok ? later.revision : null,
  savedAt: clock(),
  baseKnown: true,
});
await quire.recover({
  path: "Tasks/Today.md",
  text: "- [ ] Call back, from an older build\n",
  expectedRevision: null,
  savedAt: clock(),
  baseKnown: false,
});
await engine.publish();

// Backups of both notebooks, as Back up now makes them.
const backups = new Backups(appData);
await backups.create(folder, backupRoot, "current");
await backups.create(quire, backupRoot, "work");

// Profiles and the files beside them.
const profiles = new Profiles(appData);
profiles.migrate(`${placeholder}/folder-notebook`);
profiles.setRoot("work", `${placeholder}/quire-notebook`);
profiles.setRoot("personal", `${placeholder}/pending-notebook`);
profiles.setBackupDirectory("current", `${placeholder}/backups`);
profiles.setBackupDirectory("work", `${placeholder}/backups`);
profiles.setGitRemote("personal", "git@example.com:notes.git");
profiles.backupResult("current", { at: clock(), path: `${placeholder}/backups/Quire/current/latest` });
profiles.gitResult("personal", { error: "Git push failed: remote unavailable." });
profiles.activate("work");
profiles.close();
writeFileSync(path.join(appData, "settings.json"), JSON.stringify({ root: `${placeholder}/quire-notebook` }));
writeFileSync(path.join(appData, "appearance.json"), JSON.stringify({ appearance: "dark" }));
writeFileSync(
  path.join(appData, "ai-settings.json"),
  JSON.stringify({ enabled: true, tested: true, backend: "codex", paths: { codex: "/opt/bin/codex" }, model: "gpt-5", writes: "apply", terminal: { claude: "2.1.312" } }),
);

// A second Quire notebook whose last save never reached its synced folder: nothing awaits
// between that save and closing, so no publication timer can fire.
await createQuireNotebook(pendingRoot);
const pending = await NotebookEngine.open({ folder: pendingRoot, appData, label: "office", app: "0.9.57", now: clock });
const published = pending.save("Inbox.md", "- published\n", null);
await pending.publish();
pending.save("Inbox.md", "- published\n- not yet published\n", published.ok ? published.revision : null);

folder.close();
engine.store.close();
pending.store.close();
rmSync(output, { recursive: true, force: true });
for (const name of ["app-data", "folder-notebook", "quire-notebook", "pending-notebook", "backups"])
  cpSync(path.join(work, name), path.join(output, name), { recursive: true });
rmSync(work, { recursive: true, force: true });
console.log(`Wrote ${output}`);
process.exit(0);
