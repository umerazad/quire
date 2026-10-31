// Downloads the pinned PDFium build for this platform from bblanchon/pdfium-binaries, checks its
// SHA-256, and unpacks it where quire_media looks (crates/quire_media/src/pdf/library.rs).
// Run once per machine before building or testing: `bun tools/fetch-pdfium.ts [platform]`.
// Tests never download; they fail with this command in the message when the library is absent.
import { createHash } from "node:crypto";
import { existsSync, mkdirSync, mkdtempSync, renameSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { spawnSync } from "node:child_process";
import { archives, build, cacheDir, hostPlatform, libraryPath } from "./pdfium.ts";

const platform = process.argv[2] ?? hostPlatform();
const expected = archives[platform];
if (!expected) {
  console.error(`Unknown platform ${platform}. Pass one of: ${Object.keys(archives).join(", ")}.`);
  process.exit(1);
}
const target = cacheDir(platform);
const library = libraryPath(platform);
if (existsSync(library)) {
  console.log(`PDFium ${build} for ${platform} is already at ${library}.`);
  process.exit(0);
}

const url = `https://github.com/bblanchon/pdfium-binaries/releases/download/${encodeURIComponent(build)}/pdfium-${platform}.tgz`;
const response = await fetch(url);
if (!response.ok) {
  console.error(`Downloading ${url} failed with HTTP ${response.status}. Check the network and retry.`);
  process.exit(1);
}
const bytes = new Uint8Array(await response.arrayBuffer());
const actual = createHash("sha256").update(bytes).digest("hex");
if (actual !== expected) {
  console.error(`${url} has SHA-256 ${actual}, expected ${expected}. Do not use it; report the mismatch.`);
  process.exit(1);
}
mkdirSync(join(target, ".."), { recursive: true });
const scratch = mkdtempSync(join(target, "..", ".fetch-"));
const archive = join(scratch, "pdfium.tgz");
writeFileSync(archive, bytes);
const unpacked = join(scratch, "unpacked");
mkdirSync(unpacked);
const tar = spawnSync("tar", ["-xzf", archive, "-C", unpacked], { stdio: "inherit" });
if (tar.status !== 0) {
  console.error("Unpacking the PDFium archive failed. Check that `tar` is installed, then retry.");
  process.exit(1);
}
rmSync(target, { recursive: true, force: true });
renameSync(unpacked, target);
rmSync(scratch, { recursive: true, force: true });
console.log(`Installed PDFium ${build} for ${platform} at ${target}.`);
