import { promises as fs, constants } from "node:fs";
import path from "node:path";
import { randomUUID } from "node:crypto";

export async function syncDirectory(directory: string) {
  try {
    const handle = await fs.open(directory, "r");
    try {
      await handle.sync();
    } finally {
      await handle.close();
    }
  } catch (error) {
    if (
      process.platform !== "win32" ||
      !["EACCES", "EPERM", "EBADF", "EINVAL", "EISDIR"].includes(
        (error as NodeJS.ErrnoException).code ?? "",
      )
    )
      throw error;
  }
}
export async function publishExclusive(source: string, destination: string) {
  try {
    await fs.link(source, destination);
  } catch (error) {
    if (
      !["ENOTSUP", "EXDEV", "EPERM"].includes(
        (error as NodeJS.ErrnoException).code ?? "",
      )
    )
      throw error;
    await fs.copyFile(source, destination, constants.COPYFILE_EXCL);
    const handle = await fs.open(destination, "r+");
    try {
      await handle.sync();
    } finally {
      await handle.close();
    }
  }
}
export async function writeFileExclusive(
  destination: string,
  text: string | Uint8Array,
) {
  const temp = path.join(
    path.dirname(destination),
    `.quire-${randomUUID()}.tmp`,
  );
  try {
    const handle = await fs.open(temp, "wx", 0o600);
    try {
      await handle.writeFile(text, "utf8");
      await handle.sync();
    } finally {
      await handle.close();
    }
    await publishExclusive(temp, destination);
    await syncDirectory(path.dirname(destination));
  } finally {
    await fs.rm(temp, { force: true });
  }
}
// Replaces a file in one rename after its content is on disk, so readers
// see the old file or the new one, never part of either.
export async function writeFileReplacing(
  destination: string,
  data: string | Uint8Array,
) {
  const temp = path.join(
    path.dirname(destination),
    `.quire-${randomUUID()}.tmp`,
  );
  try {
    const handle = await fs.open(temp, "wx", 0o600);
    try {
      await handle.writeFile(data);
      await handle.sync();
    } finally {
      await handle.close();
    }
    await fs.rename(temp, destination);
    await syncDirectory(path.dirname(destination));
  } finally {
    await fs.rm(temp, { force: true });
  }
}
