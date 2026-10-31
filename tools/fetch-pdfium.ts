// Downloads the pinned PDFium build for this platform from bblanchon/pdfium-binaries, checks its
// SHA-256, and unpacks it where quire_media looks (crates/quire_media/src/pdf/library.rs).
// Run once per machine before building or testing: `bun tools/fetch-pdfium.ts [platform]`.
// Tests never download; they fail with this command in the message when the library is absent.
import { createHash } from "node:crypto";
import { existsSync, mkdirSync, mkdtempSync, renameSync, rmSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";
import { spawnSync } from "node:child_process";

// Must match PDFIUM_BUILD and the `pdfium_7881` feature of pdfium-render in crates/quire_media.
const build = "chromium/7881";
const archives: Record<string, string> = {
  "mac-arm64": "52e94ca5aa8847934330daf3f8150c190682c5ca93831468794f8b90d4392e40",
  "mac-x64": "6dedf83990e0e3d6b7c93c9e7589c5a126b0ae14b7464d76120cff7a26afb18b",
  "linux-x64": "1470e21b8b4a3b4ad7f85684e2da11d94f3b69a86d81dee11b9b6709d927ac1d",
  "linux-arm64": "ee7f7b7d5468958336a818c1cd580bdd20972846b7377b13f9a923d92d1d4674",
  "win-x64": "73cc0de638ac2095e7445bf56a38200a5b7c7ca0e9f4ba144598f2457377ac08",
  "win-arm64": "d3035d4d2cacac6ecd1a2ece197a3d702a1b2a58466276b9f870b8cb278a9d84",
};

const arm = process.arch === "arm64";
const host =
  process.platform === "darwin"
    ? arm ? "mac-arm64" : "mac-x64"
    : process.platform === "win32"
      ? arm ? "win-arm64" : "win-x64"
      : arm ? "linux-arm64" : "linux-x64";
const platform = process.argv[2] ?? host;
const expected = archives[platform];
if (!expected) {
  console.error(`Unknown platform ${platform}. Pass one of: ${Object.keys(archives).join(", ")}.`);
  process.exit(1);
}

const cacheBase =
  process.env.QUIRE_PDFIUM_DIR === undefined
    ? process.platform === "darwin"
      ? join(homedir(), "Library/Caches")
      : process.platform === "win32"
        ? process.env.LOCALAPPDATA ?? join(homedir(), "AppData/Local")
        : process.env.XDG_CACHE_HOME ?? join(homedir(), ".cache")
    : undefined;
const target =
  cacheBase === undefined
    ? process.env.QUIRE_PDFIUM_DIR!
    : join(cacheBase, "quire/pdfium", build.replace("/", "-"), platform);
const library = join(
  target,
  platform.startsWith("win") ? "bin/pdfium.dll" : platform.startsWith("mac") ? "lib/libpdfium.dylib" : "lib/libpdfium.so",
);
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
