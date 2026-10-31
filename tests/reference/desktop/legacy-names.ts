import { cpSync, existsSync, lstatSync, renameSync, rmSync } from "node:fs";
import fs from "node:fs/promises";
import path from "node:path";

// Quire was called Threadnote until 0.9.23. These move data saved under the
// old name once and never overwrite data saved under the new one.
const legacyName = "Threadnote";
const legacyMetadata = ".threadnote";

const skipped = new Set([
  "Cache",
  "Code Cache",
  "GPUCache",
  "DawnGraphiteCache",
  "DawnWebGPUCache",
  "Crashpad",
  "lockfile",
]);
// Copies the old app-data folder (settings, profiles, device id, renderer
// storage) so an older Threadnote keeps its own. Caches and locks stay.
export function adoptLegacyAppData(appData: string, name: string) {
  const from = path.join(appData, legacyName),
    to = path.join(appData, name);
  if (existsSync(to) || !existsSync(from)) return false;
  const staging = `${to}.moving`;
  rmSync(staging, { recursive: true, force: true });
  cpSync(from, staging, {
    recursive: true,
    filter: (source) => {
      const top = path.relative(from, source).split(path.sep)[0];
      return !skipped.has(top) && !top.startsWith("Singleton");
    },
  });
  renameSync(staging, to);
  return true;
}

// Renames a notebook's .threadnote folder the first time Quire opens it.
// Returns a warning when both folders exist, which means an older Threadnote
// on another device still writes the old one.
export function adoptLegacyMetadata(root: string, metadata: string) {
  const from = path.join(root, legacyMetadata),
    to = path.join(root, metadata);
  try {
    if (!lstatSync(from).isDirectory()) return;
  } catch {
    return;
  }
  if (!existsSync(to)) {
    renameSync(from, to);
    return;
  }
  return `This notebook has a ${legacyMetadata} folder from an older Threadnote beside ${metadata}. Update every device to Quire; Quire does not read ${legacyMetadata}, so that device's recovery data and history stay there until it updates.`;
}

// Renames <destination>/Threadnote, where earlier backups live, to the new
// name when no backups exist under it yet.
export async function adoptLegacyBackups(destination: string, name: string) {
  const from = path.join(destination, legacyName),
    to = path.join(destination, name);
  const legacy = await fs.lstat(from).catch(() => undefined);
  if (legacy?.isDirectory() && !existsSync(to)) await fs.rename(from, to);
}
