import { promises as fs } from "node:fs";
import path from "node:path";
import {
  syncDirectory,
  writeFileExclusive,
  writeFileReplacing,
} from "../durable-files";
import type { Folder } from "../../sync/replica";

// The synced folder on this computer's disk. Change files are written
// once, under a new name, and never replaced; only status.json changes.
export class DiskFolder implements Folder {
  constructor(readonly root: string) {}
  private resolve(relative: string) {
    const target = path.resolve(this.root, ...relative.split("/"));
    if (target !== this.root && !target.startsWith(this.root + path.sep))
      throw new Error(
        `"${relative}" is outside the notebook folder. Report this as a bug.`,
      );
    return target;
  }
  async list(directory: string) {
    try {
      return (await fs.readdir(this.resolve(directory))).filter(
        (name) => !name.startsWith("."),
      );
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code === "ENOENT") return [];
      throw error;
    }
  }
  // Unreadable files are treated as not yet delivered: a cloud placeholder
  // can fail to read while the provider downloads it.
  async read(relative: string) {
    try {
      return new Uint8Array(await fs.readFile(this.resolve(relative)));
    } catch {
      return undefined;
    }
  }
  async exists(relative: string) {
    try {
      await fs.lstat(this.resolve(relative));
      return true;
    } catch {
      return false;
    }
  }
  async write(relative: string, data: Uint8Array) {
    const target = this.resolve(relative);
    await this.ensureDirectory(path.dirname(target));
    if (path.basename(target) === "status.json")
      await writeFileReplacing(target, data);
    else
      try {
        await writeFileExclusive(target, data);
      } catch (error) {
        // The same content-named file is already there.
        if ((error as NodeJS.ErrnoException).code !== "EEXIST") throw error;
      }
  }
  private async ensureDirectory(directory: string) {
    const missing: string[] = [];
    for (let current = directory; current !== this.root;) {
      try {
        await fs.lstat(current);
        break;
      } catch {
        missing.unshift(current);
        current = path.dirname(current);
      }
    }
    for (const folder of missing) {
      await fs.mkdir(folder).catch((error: NodeJS.ErrnoException) => {
        if (error.code !== "EEXIST") throw error;
      });
      await syncDirectory(path.dirname(folder));
    }
  }
}
