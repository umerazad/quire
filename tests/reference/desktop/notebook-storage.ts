import {
  mkdirSync,
  readFileSync,
  writeFileSync,
  lstatSync,
  existsSync,
  openSync,
  fsyncSync,
  closeSync,
  linkSync,
  copyFileSync,
  unlinkSync,
  constants,
  readdirSync,
} from "node:fs";
import path from "node:path";
import { randomUUID, createHash } from "node:crypto";
import { DatabaseSync } from "node:sqlite";
import { adoptLegacyMetadata } from "./legacy-names";

export const storageHash = (value: string) =>
  createHash("sha256").update(value).digest("hex");
export function syncDirectorySync(directory: string) {
  let handle: number | undefined;
  try {
    handle = openSync(directory, "r");
    fsyncSync(handle);
  } catch (error) {
    if (
      process.platform !== "win32" ||
      !["EACCES", "EPERM", "EBADF", "EINVAL", "EISDIR"].includes(
        (error as NodeJS.ErrnoException).code ?? "",
      )
    )
      throw error;
  } finally {
    if (handle !== undefined) closeSync(handle);
  }
}
export function syncFileSync(file: string) {
  const handle = openSync(file, "r+");
  try {
    fsyncSync(handle);
  } finally {
    closeSync(handle);
  }
}
function directoryBelow(parent: string, name: string) {
  const directory = path.join(parent, name);
  try {
    mkdirSync(directory, { mode: 0o700 });
    syncDirectorySync(parent);
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code !== "EEXIST") throw error;
  }
  const stat = lstatSync(directory);
  if (stat.isSymbolicLink() || !stat.isDirectory())
    throw new Error(
      "Notebook metadata must be an ordinary local folder. Replace linked .quire folders before reopening.",
    );
  return directory;
}
// This computer's identity, shared by every notebook it opens.
export function localDeviceId(appData: string) {
  mkdirSync(appData, { recursive: true, mode: 0o700 });
  const identity = path.join(appData, "device-id");
  if (!existsSync(identity)) {
    try {
      writeFileSync(identity, randomUUID() + "\n", { flag: "wx", mode: 0o600 });
      syncFileSync(identity);
      syncDirectorySync(appData);
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code !== "EEXIST") throw error;
    }
  }
  const deviceId = readFileSync(identity, "utf8").trim();
  if (!/^[a-f0-9-]{36}$/.test(deviceId))
    throw new Error(
      "The local device identity is invalid. Restore the device-id file in app data before reopening notebooks.",
    );
  return deviceId;
}
export function notebookStorage(root: string, appData: string) {
  const deviceId = localDeviceId(appData);
  const legacyWarning = adoptLegacyMetadata(root, ".quire");
  const metadata = directoryBelow(root, ".quire"),
    devices = directoryBelow(metadata, "devices"),
    directory = directoryBelow(devices, deviceId);
  for (const name of ["durability.sqlite", "search.sqlite"]) {
    const file = path.join(directory, name);
    if (existsSync(file) && lstatSync(file).isSymbolicLink())
      throw new Error(
        "Notebook databases must be ordinary files. Restore linked databases before opening this notebook.",
      );
  }
  return {
    deviceId,
    warnings: legacyWarning ? [legacyWarning] : [],
    metadata,
    devices,
    directory,
    database: path.join(directory, "durability.sqlite"),
    search: path.join(directory, "search.sqlite"),
    legacyDatabase: path.join(
      appData,
      "durability",
      `${storageHash(root)}.sqlite`,
    ),
  };
}
export function snapshotDatabaseSync(source: string, destination: string) {
  if (lstatSync(source).isSymbolicLink())
    throw new Error(
      "A recovery database is a symbolic link. Preserve it and restore an ordinary database before retrying.",
    );
  const temporary = path.join(
    path.dirname(destination),
    `.database-${randomUUID()}.partial`,
  );
  try {
    const original = new DatabaseSync(source, { readOnly: true });
    try {
      original.prepare("VACUUM INTO ?").run(temporary);
    } finally {
      original.close();
    }
    const check = new DatabaseSync(temporary);
    try {
      check.exec("PRAGMA journal_mode=DELETE");
      if (
        Object.values(
          check.prepare("PRAGMA integrity_check").get() ?? {},
        )[0] !== "ok"
      )
        throw new Error(
          "Recovery database migration failed integrity checks. Keep the original database and retry from a verified backup.",
        );
    } finally {
      check.close();
    }
    syncFileSync(temporary);
    try {
      linkSync(temporary, destination);
    } catch (error) {
      if (
        !["ENOTSUP", "EXDEV", "EPERM"].includes(
          (error as NodeJS.ErrnoException).code ?? "",
        )
      )
        throw error;
      copyFileSync(temporary, destination, constants.COPYFILE_EXCL);
      syncFileSync(destination);
    }
    syncDirectorySync(path.dirname(destination));
  } finally {
    if (existsSync(temporary)) unlinkSync(temporary);
  }
}
export function migrateLegacyStorage(
  root: string,
  appData: string,
  storage: ReturnType<typeof notebookStorage>,
) {
  if (!existsSync(storage.database) && existsSync(storage.legacyDatabase))
    snapshotDatabaseSync(storage.legacyDatabase, storage.database);
  const marker = path.join(storage.directory, "legacy-migrated");
  if (existsSync(marker)) return;
  for (const kind of ["transactions", "recovery"]) {
    const old = path.join(appData, kind, storageHash(root));
    if (!existsSync(old)) continue;
    if (lstatSync(old).isSymbolicLink())
      throw new Error(
        "Legacy recovery storage is linked. Restore its ordinary folder before migrating.",
      );
    const target = directoryBelow(storage.directory, kind);
    for (const name of readdirSync(old)) {
      if (!name.endsWith(".json")) continue;
      const source = path.join(old, name),
        destination = path.join(target, name);
      if (lstatSync(source).isSymbolicLink())
        throw new Error(
          "A legacy recovery record is linked. Restore it before migrating.",
        );
      if (!existsSync(destination)) {
        copyFileSync(source, destination, constants.COPYFILE_EXCL);
        syncFileSync(destination);
      }
    }
    syncDirectorySync(target);
  }
  writeFileSync(marker, "Legacy app-data copies retained.\n", {
    flag: "wx",
    mode: 0o600,
  });
  syncFileSync(marker);
  syncDirectorySync(storage.directory);
}
